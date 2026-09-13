//! Owned staging directory and publication without replacing an existing name.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct WorkingOutput {
    pub(super) path: PathBuf,
    directory: PathBuf,
}

impl WorkingOutput {
    pub(super) fn new(output: &Path) -> io::Result<Self> {
        let parent = output.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for _ in 0..32 {
            let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!(".oxide-part-{}-{nonce}-{id}", std::process::id()));
            match Self::reserve(directory, output) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                result => return result,
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Не удалось выделить временную папку конвертации",
        ))
    }

    fn reserve(directory: PathBuf, output: &Path) -> io::Result<Self> {
        let name = output.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        // Acquire ownership atomically. In particular, never adopt or clean up an
        // existing candidate (including a symlink) after FFmpeg's -n rejects it.
        fs::create_dir(&directory)?;
        Ok(Self {
            path: directory.join(name),
            directory,
        })
    }
}

impl Drop for WorkingOutput {
    fn drop(&mut self) {
        super::remove_generated_output(&self.path);
        // Deliberately non-recursive: unexpected contents must not be deleted.
        if let Err(error) = fs::remove_dir(&self.directory)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "Could not remove conversion staging directory {}: {error}",
                self.directory.display()
            );
        }
    }
}

pub(super) fn publish_output(working: &Path, output: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(working)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Результат конвертации не является обычным файлом",
        ));
    }
    // FFmpeg has exited and closed its output. A failed flush must not publish a
    // success. This is not a guarantee against faulty hardware or power loss.
    fs::OpenOptions::new()
        .write(true)
        .open(working)?
        .sync_all()?;
    publish_name(working, output)
}

#[cfg(windows)]
fn publish_name(working: &Path, output: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    // Canonicalize only the destination's parent, never an existing destination
    // symlink. Windows canonical paths also support Unicode and long paths.
    let source = fs::canonicalize(working)?;
    let destination = fs::canonicalize(output.parent().ok_or(io::ErrorKind::InvalidInput)?)?
        .join(output.file_name().ok_or(io::ErrorKind::InvalidInput)?);
    let wide = |path: &Path| -> io::Result<Vec<u16>> {
        let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        value.push(0);
        Ok(value)
    };
    let source = wide(&source)?;
    let destination = wide(&destination)?;
    // SAFETY: both paths are live, NUL-terminated UTF-16 buffers. Neither
    // REPLACE_EXISTING nor COPY_ALLOWED is set: an occupied name must fail, and
    // moving across volumes must not fall back to a partially visible copy.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn publish_name(working: &Path, output: &Path) -> io::Result<()> {
    // Creating a hard link is an atomic, no-clobber name publication on Unix.
    // WorkingOutput drops the private link afterwards. Unsupported filesystems
    // fail safely; never fall back to rename (replaces) or copy (partial file).
    fs::hard_link(working, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_never_adopts_existing_directory_or_its_contents() {
        let files = super::super::lifecycle_tests::TestFiles::new();
        let directory = files.path("occupied");
        fs::create_dir(&directory).unwrap();
        let existing = directory.join("result.mp4");
        fs::write(&existing, b"not ours").unwrap();
        assert!(WorkingOutput::reserve(directory, Path::new("result.mp4")).is_err());
        assert_eq!(fs::read(existing).unwrap(), b"not ours");
    }
}
