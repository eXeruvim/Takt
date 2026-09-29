use super::*;
use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) struct TestDirectory(PathBuf);

impl TestDirectory {
    pub(crate) fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "oxide-storage-test-{}-{nonce}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        // Exclusively created disposable fixture; never a user data directory.
        fs::remove_dir_all(&self.0).expect("remove owned test directory");
    }
}

const FIRST: &str = "STATE_V1\nfirst";
const SECOND: &str = "STATE_V1\nsecond";

fn parse(content: &str) -> io::Result<String> {
    if content.starts_with("STATE_V1\n") {
        Ok(content.to_owned())
    } else if content.starts_with("STATE_V") {
        Err(io::ErrorKind::Unsupported.into())
    } else {
        Err(io::ErrorKind::InvalidData.into())
    }
}

fn open(path: &Path) -> (Persistence, String) {
    Persistence::open(path, parse).unwrap()
}

fn assert_no_temps(files: &TestDirectory) {
    assert!(fs::read_dir(&files.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".oxide-save-")
    }));
}

#[test]
fn legacy_data_migrates_without_changing_its_schema_and_keeps_backup() {
    let files = TestDirectory::new();
    let path = files.path("данные.txt");
    fs::write(&path, FIRST).unwrap();
    let (mut store, value) = open(&path);
    assert_eq!(value, FIRST);
    store.save(SECOND).unwrap();
    assert_eq!(open(&path).1, SECOND);
    assert_eq!(fs::read(sidecar(&path, ".bak")).unwrap(), FIRST.as_bytes());
    assert_no_temps(&files);
}

#[test]
fn first_save_has_a_recovery_copy_and_missing_primary_can_be_repaired() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    open(&path).0.save(FIRST).unwrap();
    fs::remove_file(&path).unwrap();
    let (mut recovered, value) = open(&path);
    assert_eq!(value, FIRST);
    assert!(recovered.needs_repair());
    assert!(recovered.take_notice().is_some());
    recovered.save(&value).unwrap();
    assert!(!open(&path).0.needs_repair());
}

#[test]
fn checksum_corruption_recovers_previous_version_without_poisoning_backup() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    store.save(FIRST).unwrap();
    store.save(SECOND).unwrap();
    let backup = fs::read(sidecar(&path, ".bak")).unwrap();
    let mut damaged = fs::read(&path).unwrap();
    *damaged.last_mut().unwrap() ^= 1;
    fs::write(&path, &damaged).unwrap();
    let (mut recovered, value) = open(&path);
    assert_eq!(value, FIRST);
    assert_eq!(fs::read(&path).unwrap(), damaged); // read did not mutate files
    recovered.save(&value).unwrap();
    assert_eq!(fs::read(sidecar(&path, ".bak")).unwrap(), backup);
    assert_eq!(open(&path).1, FIRST);
}

#[test]
fn integrity_detects_truncation_invalid_utf8_and_extra_bytes() {
    let bytes = encode(FIRST).unwrap();
    for length in [0, 1, bytes.len() / 2, bytes.len() - 1] {
        assert!(decode(&bytes[..length]).and_then(parse).is_err());
    }
    let mut damaged = bytes.clone();
    damaged.push(b'!');
    assert!(decode(&damaged).is_err());
    assert!(decode(&[0xff]).is_err());
}

#[test]
fn unknown_versions_and_unrecoverable_corruption_are_never_overwritten() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    for bytes in [
        b"STATE_V2\nnewer".as_slice(),
        b"OXIDE_STORAGE\t2\t0\t0\n",
        b"broken",
    ] {
        fs::write(&path, bytes).unwrap();
        // A valid backup must not downgrade a file written by a future version.
        fs::write(
            sidecar(&path, ".bak"),
            if bytes == b"broken" {
                b"also broken".as_slice()
            } else {
                FIRST.as_bytes()
            },
        )
        .unwrap();
        let (mut store, value) = Persistence::load(Some(path.clone()), parse);
        assert_eq!(value, "");
        assert!(store.take_notice().is_some());
        assert!(store.save(SECOND).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn partial_write_and_disk_full_leave_old_file_intact() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    fs::write(&path, FIRST).unwrap();
    let result = atomic_write_with(&path, SECOND.as_bytes(), |file, bytes| {
        file.write_all(&bytes[..4])?;
        Err(io::ErrorKind::StorageFull.into())
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::StorageFull);
    assert_eq!(fs::read(&path).unwrap(), FIRST.as_bytes());
    assert_no_temps(&files);
}

#[test]
fn backup_failure_aborts_before_primary_and_primary_failure_can_be_retried() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    store.save(FIRST).unwrap();
    let original = fs::read(&path).unwrap();
    for fail_backup in [true, false] {
        let error = store
            .file
            .as_mut()
            .unwrap()
            .save_using(SECOND, &mut |target, bytes| {
                if target.ends_with("state.txt.bak") == fail_backup {
                    atomic_write_with(target, bytes, |file, bytes| {
                        file.write_all(&bytes[..3])?;
                        Err(io::ErrorKind::StorageFull.into())
                    })
                } else {
                    atomic_write(target, bytes)
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            decode(&fs::read(sidecar(&path, ".bak")).unwrap()).unwrap(),
            FIRST
        );
        assert_no_temps(&files);
    }
    store.save(SECOND).unwrap();
    assert_eq!(open(&path).1, SECOND);
}

#[test]
fn permission_failure_does_not_change_primary_or_claim_success() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    store.save(FIRST).unwrap();
    let original = fs::read(&path).unwrap();
    assert_eq!(
        store
            .file
            .as_mut()
            .unwrap()
            .save_using(SECOND, &mut |_, _| {
                Err(io::ErrorKind::PermissionDenied.into())
            })
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    store.save(SECOND).unwrap();
}

#[test]
fn panic_during_write_cleans_temp_and_releases_lock() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    store.save(FIRST).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store
            .file
            .as_mut()
            .unwrap()
            .save_using(SECOND, &mut |target, bytes| {
                atomic_write_with(target, bytes, |file, bytes| {
                    file.write_all(&bytes[..3])?;
                    panic!("injected write failure");
                })
            })
    }));
    assert!(result.is_err());
    assert_no_temps(&files);
    assert_eq!(open(&path).1, FIRST);
    store.save(SECOND).unwrap();
}

#[test]
fn stale_instance_and_external_edit_are_detected_before_backup_is_touched() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut first = open(&path).0;
    let mut stale = open(&path).0;
    first.save(FIRST).unwrap();
    let backup = fs::read(sidecar(&path, ".bak")).unwrap();
    assert_eq!(
        stale.save(SECOND).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(sidecar(&path, ".bak")).unwrap(), backup);
    fs::write(&path, b"external edit").unwrap();
    assert!(first.save(SECOND).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"external edit");
}

#[test]
fn oversized_input_and_output_fail_closed() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    assert!(store.save(&"x".repeat(MAX_FILE_BYTES)).is_err());
    assert!(!path.exists());
    fs::write(&path, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
    assert!(Persistence::open(&path, parse).is_err());
}

#[test]
fn unreadable_state_path_disables_saving_without_mutating_it() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    fs::create_dir(&path).unwrap();

    let (mut store, value) = Persistence::load(Some(path.clone()), parse);
    assert_eq!(value, "");
    let notice = store.take_notice().expect("read failure must be visible");
    assert!(notice.contains("сохранение отключено"));
    assert!(store.save(SECOND).is_err());
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child(files: &TestDirectory, mode: &str, label: &str) -> ChildGuard {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "storage::tests::helper_child",
            "--ignored",
            "--nocapture",
        ])
        .env("OXIDE_STORAGE_TEST_DIR", &files.0)
        .env("OXIDE_STORAGE_TEST_MODE", mode)
        .env("OXIDE_STORAGE_TEST_LABEL", label)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    ChildGuard(command.spawn().unwrap())
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "storage fixture timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "disposable storage subprocess fixture; invoked by parent tests"]
fn helper_child() {
    let root = PathBuf::from(std::env::var_os("OXIDE_STORAGE_TEST_DIR").unwrap());
    let mode = std::env::var("OXIDE_STORAGE_TEST_MODE").unwrap();
    let label = std::env::var("OXIDE_STORAGE_TEST_LABEL").unwrap();
    let path = root.join("state.txt");
    match mode.as_str() {
        "abort" => {
            let _ = atomic_write_with(&path, SECOND.as_bytes(), |file, bytes| {
                file.write_all(&bytes[..4])?;
                file.sync_all()?;
                std::process::exit(73); // no Rust destructors, deliberately
            });
        }
        "lock" => {
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(sidecar(&path, ".lock"))
                .unwrap();
            lock.try_lock().unwrap();
            fs::write(root.join("ready"), b"ready").unwrap();
            wait_until(|| root.join("release").exists());
        }
        "writer" => {
            let mut store = open(&path).0;
            fs::write(root.join(format!("{label}.ready")), b"ready").unwrap();
            wait_until(|| root.join("go").exists());
            let result = store.save(&format!("STATE_V1\n{label}"));
            fs::write(
                root.join(format!("{label}.result")),
                if result.is_ok() {
                    b"ok".as_slice()
                } else {
                    b"conflict"
                },
            )
            .unwrap();
        }
        _ => panic!("unknown storage fixture"),
    }
}

#[test]
fn process_exit_mid_write_leaves_complete_primary_and_orphan_temp_only() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    open(&path).0.save(FIRST).unwrap();
    let mut worker = child(&files, "abort", "");
    wait_until(|| worker.0.try_wait().unwrap().is_some());
    assert_eq!(worker.0.wait().unwrap().code(), Some(73));
    assert_eq!(open(&path).1, FIRST);
    assert!(fs::read_dir(&files.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".oxide-save-")
    }));
}

#[test]
fn killed_process_releases_os_lock_without_removing_sidecar() {
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    let mut worker = child(&files, "lock", "");
    wait_until(|| files.path("ready").exists());
    assert_eq!(
        store.save(FIRST).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    store.save(FIRST).unwrap();
    assert_eq!(open(&path).1, FIRST);
}

#[test]
fn two_real_processes_publish_exactly_one_complete_revision() {
    let files = TestDirectory::new();
    let mut first = child(&files, "writer", "one");
    let mut second = child(&files, "writer", "two");
    wait_until(|| files.path("one.ready").exists() && files.path("two.ready").exists());
    fs::write(files.path("go"), b"go").unwrap();
    wait_until(|| first.0.try_wait().unwrap().is_some() && second.0.try_wait().unwrap().is_some());
    assert!(first.0.wait().unwrap().success());
    assert!(second.0.wait().unwrap().success());
    let first_ok = fs::read(files.path("one.result")).unwrap() == b"ok";
    let second_ok = fs::read(files.path("two.result")).unwrap() == b"ok";
    assert_ne!(first_ok, second_ok);
    assert_eq!(
        open(&files.path("state.txt")).1,
        if first_ok {
            "STATE_V1\none"
        } else {
            "STATE_V1\ntwo"
        }
    );
    assert_no_temps(&files);
}

#[cfg(windows)]
#[test]
fn read_only_destination_is_readable_but_cannot_be_replaced() {
    struct RestorePermissions(PathBuf, fs::Permissions);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    let files = TestDirectory::new();
    let path = files.path("state.txt");
    let mut store = open(&path).0;
    store.save(FIRST).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    let restore = RestorePermissions(path.clone(), permissions.clone());
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(open(&path).1, FIRST);
    assert!(store.save(SECOND).is_err());
    assert_eq!(open(&path).1, FIRST);
    assert_no_temps(&files);
    drop(restore);
    store.save(SECOND).unwrap();
}
