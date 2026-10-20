//! Small transactional store shared by settings and playback positions.
//! The schema stays with each caller; this layer owns integrity and file updates.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const ENVELOPE: &str = "TAKT_STORAGE";
const LEGACY_ENVELOPE: &str = "OXIDE_STORAGE";
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
pub(crate) struct Persistence {
    file: Option<StateFile>,
    problem: Option<String>,
    notice: Option<String>,
    recovered: bool,
}

impl Persistence {
    pub(crate) fn load<T: Default>(
        path: Option<PathBuf>,
        parse: impl Fn(&str) -> io::Result<T>,
    ) -> (Self, T) {
        let result = path
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Папка хранения не определена"))
            .and_then(|path| Self::open(&path, parse));
        match result {
            Ok(loaded) => loaded,
            Err(error) => {
                let problem = format!(
                    "{error}. Используются временные значения; сохранение отключено, исходные файлы не изменены"
                );
                (
                    Self {
                        problem: Some(problem.clone()),
                        notice: Some(problem),
                        ..Self::default()
                    },
                    T::default(),
                )
            }
        }
    }

    pub(crate) fn open<T: Default>(
        path: &Path,
        parse: impl Fn(&str) -> io::Result<T>,
    ) -> io::Result<(Self, T)> {
        let primary = read_optional(path)?;
        let decoded = primary
            .as_deref()
            .map(|bytes| decode(bytes).and_then(&parse));
        let (value, good, recovered) = match decoded {
            Some(Ok(value)) => (value, primary.clone(), false),
            Some(Err(error)) if error.kind() != io::ErrorKind::InvalidData => return Err(error),
            failure => {
                let backup = read_optional(&sidecar(path, ".bak"))?;
                match backup {
                    Some(bytes) => {
                        let value = decode(&bytes).and_then(parse).map_err(|error| {
                            io::Error::new(
                                error.kind(),
                                format!("Резервная копия {}: {error}", path.display()),
                            )
                        })?;
                        (value, Some(bytes), true)
                    }
                    None if failure.is_none() => (T::default(), None, false),
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "Повреждён файл {}, корректной резервной копии нет",
                                path.display()
                            ),
                        ));
                    }
                }
            }
        };
        Ok((Self {
            file: Some(StateFile { path: path.to_owned(), expected: primary, good }),
            problem: None,
            notice: recovered.then(|| format!(
                "Данные {} восстановлены из резервной копии; последние изменения могли быть потеряны",
                path.display()
            )),
            recovered,
        }, value))
    }

    pub(crate) fn needs_repair(&self) -> bool {
        self.recovered
    }

    pub(crate) fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    pub(crate) fn save(&mut self, content: &str) -> io::Result<()> {
        if let Some(problem) = &self.problem {
            return Err(io::Error::other(problem.clone()));
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Хранилище не настроено"))?;
        file.save_using(content, &mut atomic_write)?;
        self.recovered = false;
        Ok(())
    }
}

#[derive(Debug)]
struct StateFile {
    path: PathBuf,
    /// Exact bytes observed on load/last commit, including damaged primary bytes
    /// when recovery selected the backup. Compare under the interprocess lock.
    expected: Option<Vec<u8>>,
    /// Last successfully parsed or committed version, never damaged input.
    good: Option<Vec<u8>>,
}

impl StateFile {
    fn save_using(
        &mut self,
        content: &str,
        write: &mut impl FnMut(&Path, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let bytes = encode(content)?;
        let parent = self.path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        fs::create_dir_all(parent)?;
        // Keep this sidecar forever. Removing it would let a second instance
        // lock a new inode while another writer still holds the old one.
        let lock_path = sidecar(&self.path, ".lock");
        reject_special_file(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                "Хранилище занято другим экземпляром Takt. Повторите сохранение",
            ),
            fs::TryLockError::Error(error) => error,
        })?;
        if read_optional(&self.path)? != self.expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Данные изменены в другом окне или программе. Перезапустите плеер перед повтором — чужие изменения не перезаписаны",
            ));
        }
        // The first save also gets a recovery copy. Later saves keep the prior
        // valid version; recovery never copies damaged primary bytes over it.
        write(
            &sidecar(&self.path, ".bak"),
            self.good.as_deref().unwrap_or(&bytes),
        )?;
        sync_directory(parent)?;
        write(&self.path, &bytes)?;
        // Publication already happened. Update the expected revision even if
        // directory sync below fails, allowing a subsequent explicit retry.
        self.expected = Some(bytes.clone());
        self.good = Some(bytes);
        sync_directory(parent)
        // Dropping File releases the OS lock on success, errors and unwind.
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn reject_special_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Ожидался обычный файл: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    reject_special_file(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Файл состояния превышает лимит 8 МиБ",
        ));
    }
    Ok(Some(bytes))
}

// FNV-1a detects accidental corruption; not an authenticity/security signature.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn encode(content: &str) -> io::Result<Vec<u8>> {
    if content.len() > MAX_FILE_BYTES - 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Данные состояния превышают лимит 8 МиБ",
        ));
    }
    Ok(format!(
        "{ENVELOPE}\t1\t{}\t{:016x}\n{content}",
        content.len(),
        checksum(content.as_bytes())
    )
    .into_bytes())
}

fn decode(bytes: &[u8]) -> io::Result<&str> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let envelope = if text.starts_with(ENVELOPE) {
        ENVELOPE
    } else if text.starts_with(LEGACY_ENVELOPE) {
        LEGACY_ENVELOPE
    } else {
        return Ok(text); // Existing versioned schemas remain readable.
    };
    let (header, content) = text.split_once('\n').ok_or(io::ErrorKind::InvalidData)?;
    let fields: Vec<_> = header.splitn(5, '\t').collect();
    if fields.len() != 4 || fields[0] != envelope {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if fields[1] != "1" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Файл записан другой версией хранилища Takt",
        ));
    }
    if fields[2].parse::<usize>().ok() != Some(content.len())
        || u64::from_str_radix(fields[3], 16).ok() != Some(checksum(content.as_bytes()))
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(content)
}

/// Copy a pre-rename state file into the current application directory once.
/// The old file remains untouched so a rollback to an older build is safe.
pub(crate) fn migrate_legacy_path(preferred: PathBuf, legacy: PathBuf) -> PathBuf {
    if preferred.is_file() || sidecar(&preferred, ".bak").is_file() {
        return preferred;
    }

    let legacy_backup = sidecar(&legacy, ".bak");
    if !legacy.is_file() && !legacy_backup.is_file() {
        return preferred;
    }

    let Some(parent) = preferred.parent() else {
        return legacy;
    };
    if fs::create_dir_all(parent).is_err() {
        return legacy;
    }

    let mut copied_any = false;
    for (source, destination) in [
        (&legacy, preferred.clone()),
        (&legacy_backup, sidecar(&preferred, ".bak")),
    ] {
        let bytes = match read_optional(source) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(_) => return legacy.clone(),
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
        {
            Ok(mut file) => {
                if file
                    .write_all(&bytes)
                    .and_then(|()| file.sync_all())
                    .is_err()
                {
                    let _ = fs::remove_file(&destination);
                    return legacy.clone();
                }
                copied_any = true;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                copied_any = true;
            }
            Err(_) => return legacy.clone(),
        }
    }

    if copied_any { preferred } else { legacy }
}

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not remove storage temporary file {}: {error}",
                self.0.display()
            );
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with(path, bytes, |file, bytes| file.write_all(bytes))
}

fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    reject_special_file(path)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut reserved = None;
    for _ in 0..32 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp = sidecar(
            path,
            &format!(".takt-save-{}-{nonce}-{id}.tmp", std::process::id()),
        );
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => {
                reserved = Some((TemporaryFile(temp), file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (temp, mut file) = reserved.ok_or(io::ErrorKind::AlreadyExists)?;
    // File is dropped before the temporary-path guard, including on errors/panic.
    write(&mut file, bytes)?;
    file.sync_all()?;
    drop(file);
    replace(&temp.0, path)
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = fs::canonicalize(source)?;
    let destination = fs::canonicalize(destination.parent().ok_or(io::ErrorKind::InvalidInput)?)?
        .join(destination.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    let wide = |path: &Path| -> io::Result<Vec<u16>> {
        let mut bytes: Vec<_> = path.as_os_str().encode_wide().collect();
        if bytes.contains(&0) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        bytes.push(0);
        Ok(bytes)
    };
    let source = wide(&source)?;
    let destination = wide(&destination)?;
    // SAFETY: live NUL-terminated paths; same-directory temp, no copy fallback.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    } // Windows publication uses WRITE_THROUGH.
}

#[cfg(test)]
pub(crate) mod tests;
