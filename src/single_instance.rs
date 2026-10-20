//! Windows single-instance coordination for user-facing release builds.
//!
//! A second launch forwards its path to the existing process through a
//! session-local named pipe, then exits. Diagnostic command-line probes bypass
//! this module in `main` and therefore remain scriptable while Takt is open.

#![deny(clippy::undocumented_unsafe_blocks)]

use std::{
    ffi::OsString,
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr, thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, bounded};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GENERIC_WRITE, GetLastError,
        HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    },
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
        },
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{CreateMutexW, GetCurrentProcessId},
    },
    UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
};

const PROTOCOL_MAGIC: [u8; 4] = *b"TAK1";
const HEADER_BYTES: usize = 12;
const MAX_PATH_UNITS: usize = 32_767;
const MAX_MESSAGE_BYTES: usize = HEADER_BYTES + MAX_PATH_UNITS * 2;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);
const INCOMING_CAPACITY: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LaunchRequest {
    Activate,
    OpenFile(PathBuf),
    OpenFolder(PathBuf),
}

pub(crate) enum Acquisition {
    Primary(PrimaryInstance),
    Forwarded,
}

pub(crate) struct PrimaryInstance {
    _mutex: OwnedHandle,
    incoming: Receiver<LaunchRequest>,
}

impl PrimaryInstance {
    pub(crate) fn incoming(&self) -> Receiver<LaunchRequest> {
        self.incoming.clone()
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `OwnedHandle` is constructed only from a successful Win32
        // handle-returning call and owns that handle exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Endpoint {
    mutex: Vec<u16>,
    pipe: Vec<u16>,
}

impl Endpoint {
    fn current() -> io::Result<Self> {
        let mut session = 0;
        // SAFETY: `session` is valid writable storage and the current process
        // ID always identifies the calling process for this synchronous call.
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            mutex: wide_null(&format!("Local\\Takt.MediaPlayer.v1.{session}")),
            pipe: wide_null(&format!(r"\\.\pipe\Takt.MediaPlayer.v1.{session}")),
        })
    }
}

pub(crate) fn acquire(request: LaunchRequest) -> Result<Acquisition> {
    let endpoint = Endpoint::current().context("Не удалось определить сеанс Windows")?;
    // SAFETY: Both optional pointers are null as permitted by CreateMutexW;
    // `endpoint.mutex` is NUL-terminated and lives through the call.
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, endpoint.mutex.as_ptr()) };
    if mutex.is_null() {
        return Err(io::Error::last_os_error()).context("Не удалось создать блокировку Takt");
    }
    // SAFETY: GetLastError has no pointer arguments and must be observed
    // immediately after CreateMutexW; ERROR_ALREADY_EXISTS identifies the
    // secondary process when the returned handle itself is valid.
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle(mutex);

    if already_running {
        // The user initiated this process, so let the receiving Takt process
        // request foreground activation when it handles the message.
        // SAFETY: ASFW_ANY is the documented sentinel; failure is harmless and
        // Windows may still flash the taskbar button instead of stealing focus.
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
        forward(&endpoint.pipe, &request)?;
        drop(mutex);
        return Ok(Acquisition::Forwarded);
    }

    let (incoming_tx, incoming) = bounded(INCOMING_CAPACITY);
    let pipe_name = endpoint.pipe;
    thread::Builder::new()
        .name("takt-single-instance".into())
        .spawn(move || serve(pipe_name, incoming_tx))
        .context("Не удалось запустить single-instance канал")?;

    Ok(Acquisition::Primary(PrimaryInstance {
        _mutex: mutex,
        incoming,
    }))
}

fn serve(pipe_name: Vec<u16>, incoming: crossbeam_channel::Sender<LaunchRequest>) {
    // SAFETY: The name is NUL-terminated; null security attributes request the
    // process default DACL. The pipe rejects remote clients and is bounded to
    // one session-specific instance.
    let pipe = unsafe {
        CreateNamedPipeW(
            pipe_name.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            MAX_MESSAGE_BYTES as u32,
            FORWARD_TIMEOUT.as_millis() as u32,
            ptr::null(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return;
    }
    let pipe = OwnedHandle(pipe);
    let mut buffer = vec![0_u8; MAX_MESSAGE_BYTES];

    loop {
        // SAFETY: `pipe` is a live synchronous named-pipe server handle and a
        // null OVERLAPPED pointer requests a blocking connection.
        let connected = unsafe { ConnectNamedPipe(pipe.0, ptr::null_mut()) } != 0 || {
            // SAFETY: GetLastError has no pointer arguments and is read
            // immediately after the failed ConnectNamedPipe call.
            (unsafe { GetLastError() }) == ERROR_PIPE_CONNECTED
        };
        if !connected {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let mut read = 0_u32;
        // SAFETY: `buffer` is writable for its full declared length, `read` is
        // valid output storage, and synchronous I/O uses a null OVERLAPPED.
        let succeeded = unsafe {
            ReadFile(
                pipe.0,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        } != 0;
        if succeeded && let Ok(request) = decode(&buffer[..read as usize]) {
            let _ = incoming.try_send(request);
        }
        // SAFETY: `pipe` is connected or the peer has already disconnected;
        // DisconnectNamedPipe is valid in either case before the next accept.
        unsafe {
            DisconnectNamedPipe(pipe.0);
        }
    }
}

fn forward(pipe_name: &[u16], request: &LaunchRequest) -> Result<()> {
    let payload = encode(request)?;
    let deadline = Instant::now() + FORWARD_TIMEOUT;
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "single-instance pipe not ready");

    while Instant::now() < deadline {
        // SAFETY: `pipe_name` is NUL-terminated and all optional handles and
        // security attributes use documented null values.
        let pipe = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if pipe != INVALID_HANDLE_VALUE {
            let pipe = OwnedHandle(pipe);
            let mut written = 0_u32;
            // SAFETY: `payload` remains readable for the synchronous write,
            // `written` is valid output storage, and OVERLAPPED is null.
            let succeeded = unsafe {
                WriteFile(
                    pipe.0,
                    payload.as_ptr(),
                    payload.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            } != 0;
            if succeeded && written as usize == payload.len() {
                return Ok(());
            }
            last_error = io::Error::last_os_error();
        } else {
            last_error = io::Error::last_os_error();
        }
        thread::sleep(Duration::from_millis(25));
    }

    Err(last_error).context("Не удалось передать файл уже запущенному Takt")
}

fn encode(request: &LaunchRequest) -> Result<Vec<u8>> {
    let (kind, path) = match request {
        LaunchRequest::Activate => (0_u8, None),
        LaunchRequest::OpenFile(path) => (1, Some(path)),
        LaunchRequest::OpenFolder(path) => (2, Some(path)),
    };
    let wide = path
        .map(|path| path.as_os_str().encode_wide().collect::<Vec<_>>())
        .unwrap_or_default();
    if wide.len() > MAX_PATH_UNITS {
        bail!("Слишком длинный путь для передачи уже запущенному Takt");
    }

    let mut output = Vec::with_capacity(HEADER_BYTES + wide.len() * 2);
    output.extend_from_slice(&PROTOCOL_MAGIC);
    output.push(kind);
    output.extend_from_slice(&[0, 0, 0]);
    output.extend_from_slice(&(wide.len() as u32).to_le_bytes());
    for unit in wide {
        output.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(output)
}

fn decode(input: &[u8]) -> Result<LaunchRequest> {
    if input.len() < HEADER_BYTES || input[..4] != PROTOCOL_MAGIC {
        bail!("Некорректный single-instance пакет");
    }
    let kind = input[4];
    let units = u32::from_le_bytes(input[8..12].try_into().expect("fixed header")) as usize;
    if units > MAX_PATH_UNITS || input.len() != HEADER_BYTES + units * 2 {
        bail!("Некорректная длина single-instance пакета");
    }
    let mut wide = Vec::with_capacity(units);
    for bytes in input[HEADER_BYTES..].chunks_exact(2) {
        wide.push(u16::from_le_bytes([bytes[0], bytes[1]]));
    }
    let path = PathBuf::from(OsString::from_wide(&wide));
    match (kind, units) {
        (0, 0) => Ok(LaunchRequest::Activate),
        (1, 1..) => Ok(LaunchRequest::OpenFile(path)),
        (2, 1..) => Ok(LaunchRequest::OpenFolder(path)),
        _ => bail!("Неизвестная single-instance команда"),
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_unicode_paths() {
        let file = LaunchRequest::OpenFile(PathBuf::from(r"C:\Видео с пробелом\ёжик.mp4"));
        let folder = LaunchRequest::OpenFolder(PathBuf::from(r"D:\Музыка\Альбом"));
        assert_eq!(decode(&encode(&file).unwrap()).unwrap(), file);
        assert_eq!(decode(&encode(&folder).unwrap()).unwrap(), folder);
        assert_eq!(
            decode(&encode(&LaunchRequest::Activate).unwrap()).unwrap(),
            LaunchRequest::Activate
        );
    }

    #[test]
    fn protocol_rejects_unknown_or_truncated_messages() {
        assert!(decode(b"TAK1").is_err());
        let mut message = encode(&LaunchRequest::Activate).unwrap();
        message[4] = 9;
        assert!(decode(&message).is_err());
    }
}
