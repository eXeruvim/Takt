//! Owned FFmpeg process and nonblocking pipes. No detached reader threads.
use std::{
    io::{self, Read},
    path::Path,
    process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) const READ_BUDGET: usize = 64 * 1024;
const REAP_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    configure_hidden_process(&mut command);
    command
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

pub(super) struct ManagedChild {
    child: Child,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    cleaned: bool,
    #[cfg(unix)]
    exit_observed: bool,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ManagedChild {
    pub(super) fn spawn(mut command: Command) -> io::Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = WindowsJob::new()?;
        let child = command.spawn()?;
        // Establish the owner before any fallible setup of handles/pipes.
        let mut process = Self {
            child,
            stdout: None,
            stderr: None,
            cleaned: false,
            #[cfg(unix)]
            exit_observed: false,
            #[cfg(windows)]
            job,
        };
        #[cfg(windows)]
        process.job.assign(&process.child)?;
        process.stdout = process.child.stdout.take();
        process.stderr = process.child.stderr.take();
        #[cfg(unix)]
        {
            if let Some(pipe) = &process.stdout {
                make_nonblocking(pipe)?;
            }
            if let Some(pipe) = &process.stderr {
                make_nonblocking(pipe)?;
            }
        }
        Ok(process)
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        #[cfg(unix)]
        if status.is_some() {
            self.exit_observed = true;
        }
        Ok(status)
    }

    pub(super) fn stdout(&mut self) -> io::Result<Vec<u8>> {
        read_available(&mut self.stdout)
    }

    pub(super) fn stderr(&mut self) -> io::Result<Vec<u8>> {
        read_available(&mut self.stderr)
    }

    /// Stop the owned process group and reap the direct child, with a bounded wait.
    pub(super) fn terminate(&mut self) -> io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        if !self.exit_observed {
            // SAFETY: spawn created a new process group with this child's PID.
            // Once reaped, its ID could be reused; never signal that group then.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        // Attempt termination even if querying state fails. kill can race with
        // normal exit; try_wait below determines whether cleanup succeeded.
        let _ = self.child.kill();
        let deadline = Instant::now() + REAP_TIMEOUT;
        loop {
            if self.child.try_wait()?.is_some() {
                self.cleaned = true;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "FFmpeg did not terminate",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if let Err(error) = self.terminate() {
            eprintln!("FFmpeg cleanup failed: {error}");
        }
    }
}

#[cfg(windows)]
fn read_available<T: Read + std::os::windows::io::AsRawHandle>(
    pipe: &mut Option<T>,
) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
    let Some(reader) = pipe.as_mut() else {
        return Ok(Vec::new());
    };
    let mut available = 0;
    // SAFETY: this is a live pipe handle with a single reader. There is no other
    // pending read, so reading at most the available byte count cannot wait for data.
    let result = unsafe {
        PeekNamedPipe(
            reader.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
            *pipe = None;
            return Ok(Vec::new());
        }
        return Err(error);
    }
    let mut bytes = vec![0; (available as usize).min(READ_BUDGET)];
    if !bytes.is_empty() {
        let count = reader.read(&mut bytes)?;
        bytes.truncate(count);
        if count == 0 {
            *pipe = None;
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
fn make_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: the pipe is owned and live; fcntl changes only its status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn read_available<T: Read>(pipe: &mut Option<T>) -> io::Result<Vec<u8>> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(Vec::new());
    };
    let mut bytes = vec![0; READ_BUDGET];
    match reader.read(&mut bytes) {
        Ok(0) => {
            *pipe = None;
            Ok(Vec::new())
        }
        Ok(count) => {
            bytes.truncate(count);
            Ok(bytes)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
struct WindowsJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl WindowsJob {
    fn new() -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: unnamed job with default security; the returned handle is owned.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits matches the supplied information class and size.
        if unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY: both handles remain owned for the duration of the association.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the owned job contains only this conversion/probe and descendants.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            eprintln!(
                "FFmpeg job termination failed: {}",
                io::Error::last_os_error()
            );
        }
    }
}
