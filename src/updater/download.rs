use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use url::Url;

use super::{
    http::{self, HttpError, StreamAction, StreamResult},
    manifest::{ManifestError, PackageKind, VerifiedUpdate, verify_package_file},
};

const COMMAND_CAPACITY: usize = 4;
const EVENT_CAPACITY: usize = 64;
const PROGRESS_STEP_PERCENT: u64 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadRequest {
    pub update: VerifiedUpdate,
    pub directory: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadEvent {
    Started { downloaded: u64, total: u64 },
    Progress { downloaded: u64, total: u64 },
    Paused { downloaded: u64, total: u64 },
    Completed { path: PathBuf },
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    Busy,
    Unavailable,
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(formatter, "команда обновления уже ожидает выполнения"),
            Self::Unavailable => write!(formatter, "загрузка обновления недоступна"),
        }
    }
}

impl std::error::Error for ControlError {}

enum DownloadCommand {
    Start(Box<DownloadRequest>),
    Pause,
    Resume,
    Cancel,
}

pub struct DownloadService {
    commands: Option<Sender<DownloadCommand>>,
    events: Receiver<DownloadEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DownloadService {
    pub fn spawn() -> io::Result<Self> {
        Self::spawn_with(HttpTransport)
    }

    pub fn start(&self, request: DownloadRequest) -> Result<(), ControlError> {
        self.send(DownloadCommand::Start(Box::new(request)))
    }

    pub fn pause(&self) -> Result<(), ControlError> {
        self.send(DownloadCommand::Pause)
    }

    pub fn resume(&self) -> Result<(), ControlError> {
        self.send(DownloadCommand::Resume)
    }

    pub fn cancel(&self) -> Result<(), ControlError> {
        self.send(DownloadCommand::Cancel)
    }

    pub fn try_event(&self) -> Result<DownloadEvent, crossbeam_channel::TryRecvError> {
        self.events.try_recv()
    }

    pub fn event_receiver(&self) -> Receiver<DownloadEvent> {
        self.events.clone()
    }

    fn send(&self, command: DownloadCommand) -> Result<(), ControlError> {
        let Some(commands) = &self.commands else {
            return Err(ControlError::Unavailable);
        };
        match commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(ControlError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(ControlError::Unavailable),
        }
    }

    fn spawn_with<T>(transport: T) -> io::Result<Self>
    where
        T: RangeTransport,
    {
        let (commands, worker_commands) = bounded(COMMAND_CAPACITY);
        let (worker_events, events) = bounded(EVENT_CAPACITY);
        let worker = thread::Builder::new()
            .name("takt-update-download".to_owned())
            .spawn(move || run_worker(worker_commands, worker_events, transport))?;
        Ok(Self {
            commands: Some(commands),
            events,
            worker: Some(worker),
        })
    }
}

impl Drop for DownloadService {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

trait RangeTransport: Send + 'static {
    fn stream(
        &mut self,
        url: &Url,
        offset: u64,
        maximum_bytes: u64,
        receive: &mut dyn FnMut(&[u8]) -> io::Result<StreamAction>,
    ) -> Result<StreamResult, HttpError>;
}

struct HttpTransport;

impl RangeTransport for HttpTransport {
    fn stream(
        &mut self,
        url: &Url,
        offset: u64,
        maximum_bytes: u64,
        receive: &mut dyn FnMut(&[u8]) -> io::Result<StreamAction>,
    ) -> Result<StreamResult, HttpError> {
        http::stream_https(url, offset, maximum_bytes, receive)
    }
}

fn run_worker<T>(
    commands: Receiver<DownloadCommand>,
    events: Sender<DownloadEvent>,
    mut transport: T,
) where
    T: RangeTransport,
{
    while let Ok(command) = commands.recv() {
        match command {
            DownloadCommand::Start(request) => {
                download_one(&request, &commands, &events, &mut transport);
            }
            DownloadCommand::Pause | DownloadCommand::Resume | DownloadCommand::Cancel => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveControl {
    Pause,
    Cancel,
}

fn download_one<T>(
    request: &DownloadRequest,
    commands: &Receiver<DownloadCommand>,
    events: &Sender<DownloadEvent>,
    transport: &mut T,
) where
    T: RangeTransport,
{
    if let Err(error) = download_one_inner(request, commands, events, transport) {
        send_terminal(events, DownloadEvent::Failed(error));
    }
}

fn download_one_inner<T>(
    request: &DownloadRequest,
    commands: &Receiver<DownloadCommand>,
    events: &Sender<DownloadEvent>,
    transport: &mut T,
) -> Result<(), String>
where
    T: RangeTransport,
{
    let package = &request.update.package;
    let url = Url::parse(&package.url).map_err(|_| "Некорректный адрес пакета".to_owned())?;
    let paths = download_paths(request)?;
    prepare_directory(&request.directory).map_err(user_io_error)?;
    if ready_file_is_valid(&paths.ready, package.size, &package.sha256) {
        remove_managed_file_if_present(&paths.partial).map_err(user_io_error)?;
        send_terminal(events, DownloadEvent::Completed { path: paths.ready });
        return Ok(());
    }
    remove_managed_file_if_present(&paths.ready).map_err(user_io_error)?;

    let mut restarted_without_range = false;
    let mut restarted_after_verification_failure = false;
    let mut announced_start = false;
    loop {
        let mut file = open_partial(&paths.partial, package.size).map_err(user_io_error)?;
        let mut downloaded = file.metadata().map_err(user_io_error)?.len();
        if !announced_start {
            send_terminal(
                events,
                DownloadEvent::Started {
                    downloaded,
                    total: package.size,
                },
            );
            announced_start = true;
        }
        if downloaded == package.size {
            drop(file);
            if reset_after_verification_failure(
                &paths.partial,
                package.size,
                &package.sha256,
                &mut restarted_after_verification_failure,
            )? {
                continue;
            }
            return finalize_download(&paths, request, events);
        }

        let offset = downloaded;
        let mut control = None;
        let mut last_percent = percentage(downloaded, package.size);
        let stream = transport.stream(&url, offset, package.size - offset, &mut |chunk| {
            if control.is_none() {
                control = poll_control(commands, events);
            }
            if control.is_some() {
                return Ok(StreamAction::Stop);
            }
            file.write_all(chunk)?;
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| io::Error::other("размер загрузки переполнен"))?;
            if downloaded > package.size {
                return Err(io::Error::other(
                    "сервер прислал больше данных, чем заявлено",
                ));
            }
            let current_percent = percentage(downloaded, package.size);
            if current_percent >= last_percent.saturating_add(PROGRESS_STEP_PERCENT)
                || downloaded == package.size
            {
                let _ = events.try_send(DownloadEvent::Progress {
                    downloaded,
                    total: package.size,
                });
                last_percent = current_percent;
            }
            Ok(StreamAction::Continue)
        });

        file.flush().map_err(user_io_error)?;
        match stream {
            Err(HttpError::RangeNotSupported) if offset > 0 && !restarted_without_range => {
                file.set_len(0).map_err(user_io_error)?;
                restarted_without_range = true;
                continue;
            }
            Err(error) => return Err(error.to_string()),
            Ok(result) if !result.complete => match control {
                Some(ActiveControl::Cancel) | None => {
                    drop(file);
                    remove_managed_file_if_present(&paths.partial).map_err(user_io_error)?;
                    send_terminal(events, DownloadEvent::Cancelled);
                    return Ok(());
                }
                Some(ActiveControl::Pause) => {
                    send_terminal(
                        events,
                        DownloadEvent::Paused {
                            downloaded,
                            total: package.size,
                        },
                    );
                    match wait_while_paused(commands, events) {
                        ActiveControl::Pause => continue,
                        ActiveControl::Cancel => {
                            drop(file);
                            remove_managed_file_if_present(&paths.partial)
                                .map_err(user_io_error)?;
                            send_terminal(events, DownloadEvent::Cancelled);
                            return Ok(());
                        }
                    }
                }
            },
            Ok(_) => {
                if downloaded != package.size {
                    return Err("Сервер завершил загрузку раньше заявленного размера".to_owned());
                }
                file.sync_data().map_err(user_io_error)?;
                drop(file);
                if reset_after_verification_failure(
                    &paths.partial,
                    package.size,
                    &package.sha256,
                    &mut restarted_after_verification_failure,
                )? {
                    continue;
                }
                return finalize_download(&paths, request, events);
            }
        }
    }
}

fn poll_control(
    commands: &Receiver<DownloadCommand>,
    events: &Sender<DownloadEvent>,
) -> Option<ActiveControl> {
    match commands.try_recv() {
        Ok(DownloadCommand::Pause) => Some(ActiveControl::Pause),
        Ok(DownloadCommand::Cancel) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
            Some(ActiveControl::Cancel)
        }
        Ok(DownloadCommand::Resume) | Err(crossbeam_channel::TryRecvError::Empty) => None,
        Ok(DownloadCommand::Start(_)) => {
            let _ = events.try_send(DownloadEvent::Failed(
                "Другая загрузка обновления уже выполняется".to_owned(),
            ));
            None
        }
    }
}

fn wait_while_paused(
    commands: &Receiver<DownloadCommand>,
    events: &Sender<DownloadEvent>,
) -> ActiveControl {
    loop {
        match commands.recv() {
            Ok(DownloadCommand::Resume) => return ActiveControl::Pause,
            Ok(DownloadCommand::Cancel) | Err(_) => return ActiveControl::Cancel,
            Ok(DownloadCommand::Start(_)) => {
                let _ = events.try_send(DownloadEvent::Failed(
                    "Сначала завершите или отмените текущую загрузку".to_owned(),
                ));
            }
            Ok(DownloadCommand::Pause) => {}
        }
    }
}

struct DownloadPaths {
    partial: PathBuf,
    ready: PathBuf,
}

fn download_paths(request: &DownloadRequest) -> Result<DownloadPaths, String> {
    let hash = &request.update.package.sha256;
    if hash.len() < 16 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Некорректный SHA-256 пакета".to_owned());
    }
    let kind = match request.update.package.kind {
        PackageKind::Installer => "installer.exe",
        PackageKind::Portable => "portable.zip",
    };
    let identity = format!("{}-{}-{kind}", request.update.version, &hash[..16]);
    Ok(DownloadPaths {
        partial: request.directory.join(format!(".{identity}.part")),
        ready: request.directory.join(format!("takt-{identity}")),
    })
}

fn prepare_directory(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(io::Error::other(
            "каталог обновлений является ссылкой или reparse point",
        ));
    }
    Ok(())
}

fn open_partial(path: &Path, expected_size: u64) -> io::Result<File> {
    reject_unsafe_existing_path(path)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::other(
            "частичный пакет не является обычным файлом",
        ));
    }
    if metadata.len() > expected_size {
        file.set_len(0)?;
    }
    file.seek(SeekFrom::End(0))?;
    Ok(file)
}

fn reject_unsafe_existing_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || is_reparse_point(&metadata) => Err(
            io::Error::other("путь пакета является ссылкой или не является файлом"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn ready_file_is_valid(path: &Path, expected_size: u64, expected_hash: &str) -> bool {
    reject_unsafe_existing_path(path).is_ok()
        && path.is_file()
        && verify_package_file(path, expected_size, expected_hash).is_ok()
}

fn reset_after_verification_failure(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
    restarted: &mut bool,
) -> Result<bool, String> {
    match verify_package_file(path, expected_size, expected_hash) {
        Ok(()) => Ok(false),
        Err(ManifestError::PackageHashMismatch | ManifestError::PackageSizeMismatch)
            if !*restarted =>
        {
            reject_unsafe_existing_path(path).map_err(user_io_error)?;
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)
                .map_err(user_io_error)?;
            *restarted = true;
            Ok(true)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn finalize_download(
    paths: &DownloadPaths,
    request: &DownloadRequest,
    events: &Sender<DownloadEvent>,
) -> Result<(), String> {
    let package = &request.update.package;
    verify_package_file(&paths.partial, package.size, &package.sha256)
        .map_err(|error| error.to_string())?;
    remove_managed_file_if_present(&paths.ready).map_err(user_io_error)?;
    fs::rename(&paths.partial, &paths.ready).map_err(user_io_error)?;
    send_terminal(
        events,
        DownloadEvent::Completed {
            path: paths.ready.clone(),
        },
    );
    Ok(())
}

fn remove_managed_file_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !is_reparse_point(&metadata) => fs::remove_file(path),
        Ok(_) => Err(io::Error::other("небезопасный путь пакета обновления")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn percentage(value: u64, total: u64) -> u64 {
    value.saturating_mul(100).checked_div(total).unwrap_or(0)
}

fn send_terminal(events: &Sender<DownloadEvent>, event: DownloadEvent) {
    let _ = events.send_timeout(event, Duration::from_millis(250));
}

fn user_io_error(error: io::Error) -> String {
    format!("Не удалось сохранить обновление: {error}")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use semver::Version;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::updater::manifest::UpdatePackage;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct FakeTransport {
        data: Vec<u8>,
        reject_first_range: bool,
    }

    impl RangeTransport for FakeTransport {
        fn stream(
            &mut self,
            _url: &Url,
            offset: u64,
            maximum_bytes: u64,
            receive: &mut dyn FnMut(&[u8]) -> io::Result<StreamAction>,
        ) -> Result<StreamResult, HttpError> {
            if offset > 0 && self.reject_first_range {
                self.reject_first_range = false;
                return Err(HttpError::RangeNotSupported);
            }
            let remaining = &self.data[offset as usize..];
            if remaining.len() as u64 > maximum_bytes {
                return Err(HttpError::ResponseTooLarge);
            }
            let mut bytes_received = 0;
            for chunk in remaining.chunks(3) {
                bytes_received += chunk.len() as u64;
                if receive(chunk).map_err(HttpError::Consumer)? == StreamAction::Stop {
                    return Ok(StreamResult {
                        complete: false,
                        bytes_received,
                    });
                }
            }
            Ok(StreamResult {
                complete: true,
                bytes_received,
            })
        }
    }

    fn fixture(data: &[u8]) -> (DownloadRequest, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::current_dir()
            .expect("test working directory")
            .join("target/updater-download-tests")
            .join(format!(
                "takt-download-test-{}-{unique}-{serial}",
                std::process::id()
            ));
        let hash = hex_lower(&Sha256::digest(data));
        (
            DownloadRequest {
                update: VerifiedUpdate {
                    version: Version::new(1, 2, 3),
                    published_at: "2026-10-14T00:00:00Z".to_owned(),
                    notes: vec![],
                    package: UpdatePackage {
                        kind: PackageKind::Installer,
                        architecture: "x86_64-windows".to_owned(),
                        url: "https://github.com/example/takt/setup.exe".to_owned(),
                        sha256: hash,
                        size: data.len() as u64,
                    },
                },
                directory: directory.clone(),
            },
            directory,
        )
    }

    #[test]
    fn completes_and_publishes_only_a_hash_verified_file() {
        let data = b"verified installer payload";
        let (request, directory) = fixture(data);
        let (commands_tx, commands) = bounded(COMMAND_CAPACITY);
        let (events_tx, events) = bounded(EVENT_CAPACITY);
        let mut transport = FakeTransport {
            data: data.to_vec(),
            reject_first_range: false,
        };
        download_one(&request, &commands, &events_tx, &mut transport);
        drop(commands_tx);
        let completed = events
            .try_iter()
            .find_map(|event| match event {
                DownloadEvent::Completed { path } => Some(path),
                _ => None,
            })
            .unwrap();
        assert_eq!(fs::read(completed).unwrap(), data);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pause_resume_and_missing_range_support_preserve_exact_bytes() {
        let data = b"resume this verified payload";
        let (request, directory) = fixture(data);
        fs::create_dir_all(&directory).unwrap();
        let paths = download_paths(&request).unwrap();
        fs::write(&paths.partial, &data[..5]).unwrap();
        let (commands_tx, commands) = bounded(COMMAND_CAPACITY);
        let (events_tx, events) = bounded(EVENT_CAPACITY);
        commands_tx.send(DownloadCommand::Pause).unwrap();
        commands_tx.send(DownloadCommand::Resume).unwrap();
        let mut transport = FakeTransport {
            data: data.to_vec(),
            reject_first_range: true,
        };
        download_one(&request, &commands, &events_tx, &mut transport);
        let captured: Vec<_> = events.try_iter().collect();
        assert!(
            captured
                .iter()
                .any(|event| matches!(event, DownloadEvent::Paused { .. })),
            "captured events: {captured:?}"
        );
        let completed = captured
            .iter()
            .find_map(|event| match event {
                DownloadEvent::Completed { path } => Some(path),
                _ => None,
            })
            .unwrap();
        assert_eq!(fs::read(completed).unwrap(), data);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancellation_removes_only_the_managed_partial_file() {
        let data = b"cancelled payload";
        let (request, directory) = fixture(data);
        let paths = download_paths(&request).unwrap();
        let (commands_tx, commands) = bounded(COMMAND_CAPACITY);
        let (events_tx, events) = bounded(EVENT_CAPACITY);
        commands_tx.send(DownloadCommand::Cancel).unwrap();
        let mut transport = FakeTransport {
            data: data.to_vec(),
            reject_first_range: false,
        };
        download_one(&request, &commands, &events_tx, &mut transport);
        assert!(!paths.partial.exists());
        assert!(
            events
                .try_iter()
                .any(|event| event == DownloadEvent::Cancelled)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_complete_partial_is_replaced_once_before_publication() {
        let data = b"correct verified payload";
        let (request, directory) = fixture(data);
        fs::create_dir_all(&directory).unwrap();
        let paths = download_paths(&request).unwrap();
        fs::write(&paths.partial, vec![b'x'; data.len()]).unwrap();
        let (_commands_tx, commands) = bounded(COMMAND_CAPACITY);
        let (events_tx, events) = bounded(EVENT_CAPACITY);
        let mut transport = FakeTransport {
            data: data.to_vec(),
            reject_first_range: false,
        };
        download_one(&request, &commands, &events_tx, &mut transport);
        let completed = events
            .try_iter()
            .find_map(|event| match event {
                DownloadEvent::Completed { path } => Some(path),
                _ => None,
            })
            .unwrap();
        assert_eq!(fs::read(completed).unwrap(), data);
        fs::remove_dir_all(directory).unwrap();
    }

    fn hex_lower(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(DIGITS[usize::from(byte >> 4)] as char);
            encoded.push(DIGITS[usize::from(byte & 0x0f)] as char);
        }
        encoded
    }
}
