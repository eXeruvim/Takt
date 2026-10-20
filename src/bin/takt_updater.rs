use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitCode},
    thread,
    time::{Duration, Instant},
};

const MAX_ARCHIVE_ENTRIES: usize = 512;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const FIRST_START_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageKind {
    Installer,
    Portable,
}

struct Request {
    kind: PackageKind,
    package: PathBuf,
    target: PathBuf,
    wait_pid: u32,
    marker: PathBuf,
}

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 1 && arguments[0] == std::ffi::OsStr::new("--build-info") {
        println!(
            "{}",
            serde_json::json!({
                "schema": 1,
                "component": "takt-updater",
                "package_version": env!("CARGO_PKG_VERSION"),
            })
        );
        return ExitCode::SUCCESS;
    }
    if arguments.len() == 2
        && arguments[0] == std::ffi::OsStr::new("--update-applied")
        && std::env::var_os("TAKT_UPDATER_TEST_PROBE").as_deref()
            == Some(std::ffi::OsStr::new("enabled"))
    {
        if std::env::var_os("TAKT_UPDATER_TEST_REJECT_FIRST_START").as_deref()
            == Some(std::ffi::OsStr::new("enabled"))
        {
            return ExitCode::FAILURE;
        }
        let marker = PathBuf::from(&arguments[1]);
        let expected = state_root().join("update-probes");
        if marker.parent() == Some(expected.as_path())
            && marker.extension().and_then(|value| value.to_str()) == Some("ready")
            && fs::write(marker, env!("CARGO_PKG_VERSION")).is_ok()
        {
            return ExitCode::SUCCESS;
        }
        return ExitCode::FAILURE;
    }
    let request = match parse(arguments) {
        Ok(request) => request,
        Err(error) => {
            let _ = write_diagnostic(&error.to_string());
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = apply(&request) {
        let _ = write_diagnostic(&error.to_string());
        if process_is_running(request.wait_pid) {
            let _ = wait_for_process(request.wait_pid, Duration::from_secs(7));
        }
        if !process_is_running(request.wait_pid) {
            let executable = request.target.join("takt.exe");
            if executable.is_file() {
                let _ = Command::new(executable).spawn();
            }
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> io::Result<Request> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("apply")) {
        return Err(invalid("ожидалась команда apply"));
    }
    let kind = match arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .as_deref()
    {
        Some("installer") => PackageKind::Installer,
        Some("portable") => PackageKind::Portable,
        _ => return Err(invalid("неизвестный вид пакета")),
    };
    let package = absolute(arguments.next(), "пакет")?;
    let target = absolute(arguments.next(), "каталог установки")?;
    let wait_pid = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| invalid("некорректный PID"))?;
    let marker = absolute(arguments.next(), "маркер запуска")?;
    if arguments.next().is_some() {
        return Err(invalid("переданы лишние аргументы"));
    }
    Ok(Request {
        kind,
        package,
        target,
        wait_pid,
        marker,
    })
}

fn absolute(value: Option<OsString>, label: &str) -> io::Result<PathBuf> {
    let path = value
        .map(PathBuf::from)
        .ok_or_else(|| invalid(format!("не указан {label}")))?;
    if !path.is_absolute() {
        return Err(invalid(format!("{label} должен иметь абсолютный путь")));
    }
    Ok(path)
}

fn apply(request: &Request) -> io::Result<()> {
    validate_request(request)?;

    let parent = request
        .target
        .parent()
        .ok_or_else(|| invalid("нет родительского каталога"))?;
    let token = request.wait_pid;
    let backup = parent.join(format!(".takt-update-backup-{token}"));
    let staging = parent.join(format!(".takt-update-stage-{token}"));
    let failed = parent.join(format!(".takt-update-failed-{token}"));
    ensure_absent(&backup)?;
    ensure_absent(&staging)?;
    ensure_absent(&failed)?;

    if request.kind == PackageKind::Portable {
        extract_portable(&request.package, &staging)?;
    }

    wait_for_process(request.wait_pid, PROCESS_EXIT_TIMEOUT)?;

    fs::rename(&request.target, &backup)?;
    let installation = match request.kind {
        PackageKind::Installer => install_with_inno(&request.package, &request.target),
        PackageKind::Portable => fs::rename(&staging, &request.target),
    };
    if let Err(error) = installation {
        restore_directory(&request.target, &backup, &failed)?;
        return Err(error);
    }

    let executable = request.target.join("takt.exe");
    let child = Command::new(&executable)
        .arg("--update-applied")
        .arg(&request.marker)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            restore_directory(&request.target, &backup, &failed)?;
            return Err(error);
        }
    };

    let confirmed = match wait_for_marker(&request.marker, &mut child, FIRST_START_TIMEOUT) {
        Ok(confirmed) => confirmed,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            restore_directory(&request.target, &backup, &failed)?;
            return Err(error);
        }
    };
    if confirmed {
        if let Err(error) =
            merge_missing(&backup, &request.target, &mut 0).and_then(|()| remove_tree(&backup))
        {
            let _ = write_diagnostic(&format!(
                "Новая версия запущена, но не удалось завершить очистку резервной копии: {error}"
            ));
        }
        let _ = fs::remove_file(&request.marker);
        return Ok(());
    }

    let _ = child.kill();
    let _ = child.wait();
    restore_directory(&request.target, &backup, &failed)?;
    Err(io::Error::other(
        "новая версия не подтвердила успешный первый запуск; выполнен откат",
    ))
}

fn validate_request(request: &Request) -> io::Result<()> {
    if !request.package.is_file() || !request.target.is_dir() {
        return Err(invalid("пакет или каталог установки недоступен"));
    }
    if fs::symlink_metadata(&request.target)?
        .file_type()
        .is_symlink()
    {
        return Err(invalid("каталог установки не должен быть ссылкой"));
    }
    if !request.target.join("takt.exe").is_file() {
        return Err(invalid("в каталоге установки нет takt.exe"));
    }
    let expected_probe_root = state_root().join("update-probes");
    if request.marker.parent() != Some(expected_probe_root.as_path())
        || request.marker.extension().and_then(|value| value.to_str()) != Some("ready")
    {
        return Err(invalid(
            "маркер запуска находится вне управляемого каталога",
        ));
    }
    Ok(())
}

fn install_with_inno(package: &Path, target: &Path) -> io::Result<()> {
    let status = Command::new(package)
        .arg("/VERYSILENT")
        .arg("/SUPPRESSMSGBOXES")
        .arg("/NORESTART")
        .arg("/SP-")
        .arg(format!("/DIR={}", target.display()))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "установщик завершился с кодом {status}"
        )))
    }
}

fn extract_portable(package: &Path, staging: &Path) -> io::Result<()> {
    fs::create_dir(staging)?;
    let mut archive = zip::ZipArchive::new(File::open(package)?)?;
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(invalid("неожиданное число файлов в portable-пакете"));
    }
    let mut root: Option<OsString> = None;
    let mut extracted = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(invalid("portable-пакет содержит ссылку"));
        }
        extracted = extracted
            .checked_add(entry.size())
            .ok_or_else(|| invalid("размер пакета переполнен"))?;
        if extracted > MAX_EXTRACTED_BYTES {
            return Err(invalid("portable-пакет слишком большой после распаковки"));
        }
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| invalid("небезопасный путь в portable-пакете"))?;
        let mut components = enclosed.components();
        let Component::Normal(first) = components
            .next()
            .ok_or_else(|| invalid("пустой путь в portable-пакете"))?
        else {
            return Err(invalid("небезопасный корень portable-пакета"));
        };
        match &root {
            Some(existing) if existing != first => {
                return Err(invalid("portable-пакет содержит несколько корней"));
            }
            None => root = Some(first.to_os_string()),
            _ => {}
        }
        let relative: PathBuf = components.collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let destination = staging.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = File::create(&destination)?;
            io::copy(&mut entry, &mut output)?;
            output.flush()?;
        }
    }
    if !staging.join("takt.exe").is_file() || !staging.join("takt-updater.exe").is_file() {
        return Err(invalid("portable-пакет не содержит обязательные файлы"));
    }
    Ok(())
}

fn merge_missing(source: &Path, destination: &Path, visited: &mut usize) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        *visited += 1;
        if *visited > 4096 {
            return Err(invalid(
                "слишком много пользовательских файлов в каталоге приложения",
            ));
        }
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(invalid("каталог приложения содержит ссылку"));
        }
        let target = destination.join(entry.file_name());
        if !target.exists() {
            fs::rename(entry.path(), target)?;
        } else if file_type.is_dir() && target.is_dir() {
            merge_missing(&entry.path(), &target, visited)?;
        }
    }
    Ok(())
}

fn restore_directory(target: &Path, backup: &Path, failed: &Path) -> io::Result<()> {
    if target.exists() {
        fs::rename(target, failed)?;
    }
    fs::rename(backup, target)?;
    if failed.exists() {
        remove_tree(failed)?;
    }
    Ok(())
}

fn wait_for_marker(marker: &Path, child: &mut Child, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if marker.is_file() {
            return Ok(true);
        }
        if child.try_wait()?.is_some() {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(false)
}

#[cfg(target_os = "windows")]
fn wait_for_process(pid: u32, timeout: Duration) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    // SAFETY: OpenProcess receives a numeric PID and no borrowed pointers. The
    // returned owned handle is closed on every path below.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return Ok(());
    }
    // SAFETY: handle was returned by OpenProcess and remains valid until closed.
    let result =
        unsafe { WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128) as u32) };
    // SAFETY: handle is owned by this function and closed exactly once.
    unsafe { CloseHandle(handle) };
    if result == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Takt не закрылся вовремя",
        ))
    }
}

#[cfg(target_os = "windows")]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    // SAFETY: OpenProcess receives a numeric PID and no borrowed pointers.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: handle remains valid until it is closed immediately below.
    let state = unsafe { WaitForSingleObject(handle, 0) };
    // SAFETY: handle is owned here and closed exactly once.
    unsafe { CloseHandle(handle) };
    state == WAIT_TIMEOUT
}

#[cfg(not(target_os = "windows"))]
fn wait_for_process(_pid: u32, _timeout: Duration) -> io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn process_is_running(_pid: u32) -> bool {
    false
}

fn ensure_absent(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("служебный путь уже существует: {}", path.display()),
        ));
    }
    Ok(())
}

fn remove_tree(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(invalid("отказ удаления ссылки"));
    }
    fs::remove_dir_all(path)
}

fn write_diagnostic(message: &str) -> io::Result<()> {
    let directory = state_root();
    fs::create_dir_all(&directory)?;
    let mut file = File::create(directory.join("last-update-error.txt"))?;
    writeln!(file, "{message}")
}

fn state_root() -> PathBuf {
    if std::env::var_os("TAKT_UPDATER_TEST_PROBE").as_deref()
        == Some(std::ffi::OsStr::new("enabled"))
        && let Some(root) = std::env::var_os("TAKT_UPDATER_TEST_ROOT").map(PathBuf::from)
        && root.is_absolute()
    {
        return root;
    }
    std::env::temp_dir().join("Takt")
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::current_dir()
                .unwrap()
                .join("target/updater-tests");
            fs::create_dir_all(&root).unwrap();
            let path = root.join(format!(
                "takt-updater-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parser_rejects_relative_and_incomplete_requests() {
        assert!(
            parse(
                [
                    "apply",
                    "portable",
                    "relative.zip",
                    "C:\\Takt",
                    "4",
                    "C:\\probe.ready",
                ]
                .map(OsString::from)
            )
            .is_err()
        );
        assert!(parse(["apply", "other"].map(OsString::from)).is_err());
    }

    #[test]
    fn portable_extracts_only_the_single_enclosed_root() {
        use zip::{ZipWriter, write::SimpleFileOptions};

        let files = TestDirectory::new();
        let archive_path = files.0.join("package.zip");
        let archive = File::create(&archive_path).unwrap();
        let mut writer = ZipWriter::new(archive);
        for name in ["Takt-1/takt.exe", "Takt-1/takt-updater.exe"] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(name.as_bytes()).unwrap();
        }
        writer.finish().unwrap();

        let staging = files.0.join("stage");
        extract_portable(&archive_path, &staging).unwrap();
        assert!(staging.join("takt.exe").is_file());
        assert!(staging.join("takt-updater.exe").is_file());
        assert!(!files.0.join("Takt-1").exists());
    }

    #[test]
    fn rollback_restores_the_previous_directory_and_removes_failed_files() {
        let files = TestDirectory::new();
        let target = files.0.join("Takt");
        let backup = files.0.join("backup");
        let failed = files.0.join("failed");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&backup).unwrap();
        fs::write(target.join("new.exe"), b"new").unwrap();
        fs::write(backup.join("takt.exe"), b"old").unwrap();

        restore_directory(&target, &backup, &failed).unwrap();

        assert_eq!(fs::read(target.join("takt.exe")).unwrap(), b"old");
        assert!(!target.join("new.exe").exists());
        assert!(!backup.exists());
        assert!(!failed.exists());
    }
}
