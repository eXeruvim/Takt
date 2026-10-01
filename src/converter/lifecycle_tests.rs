use super::*;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct TestFiles(PathBuf);

impl TestFiles {
    pub(super) fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("oxide-converter-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn spec(&self) -> JobSpec {
        let source = self.path("source.mp4");
        fs::write(&source, b"test source").unwrap();
        JobSpec {
            source,
            output: self.path("result.mp4"),
            preset: ConversionPreset::Mp4H264,
            options: ConversionOptions::default(),
            duration_seconds: 10.0,
        }
    }
}

impl Drop for TestFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn helper_command(mode: &str, pid_path: &Path) -> Command {
    let mut command = converter_process(&std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "converter::lifecycle_tests::helper_child",
            "--ignored",
            "--nocapture",
        ])
        .env("OXIDE_TEST_CHILD_MODE", mode)
        .env("OXIDE_TEST_CHILD_PID", pid_path);
    command
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "test helper did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_child_exited(pid_path: &Path) {
    let pid: u32 = fs::read_to_string(pid_path).unwrap().parse().unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::WAIT_OBJECT_0,
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        // SAFETY: only waiting on our disposable helper; no foreign process mutation.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !raw.is_null() {
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 2000) },
                WAIT_OBJECT_0
            );
        }
    }
    #[cfg(unix)]
    assert_ne!(unsafe { libc::kill(pid as i32, 0) }, 0);
}

// Invoked only as a disposable subprocess by tests below. No UI, files supplied
// by users, network requests or real media are involved.
#[test]
#[ignore = "subprocess fixture; run only through lifecycle tests"]
fn helper_child() {
    let mode = std::env::var("OXIDE_TEST_CHILD_MODE").expect("fixture mode required");
    fs::write(
        std::env::var_os("OXIDE_TEST_CHILD_PID").unwrap(),
        std::process::id().to_string(),
    )
    .unwrap();
    if let Some(path) = std::env::var_os("OXIDE_TEST_CHILD_OUTPUT") {
        fs::write(path, b"partial output").unwrap();
    }
    match mode.as_str() {
        "version" => {
            println!("ffmpeg version fixture");
            std::process::exit(0);
        }
        "fail" => {
            eprint!("fixture encoder failed without newline");
            io::stderr().flush().unwrap();
            std::process::exit(7);
        }
        "success" => {
            print!("out_time_us=5000000\ntotal_size=14\n");
            io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "flood" => loop {
            io::stdout().write_all(&[b'x'; 4096]).unwrap();
        },
        "progress-flood" => loop {
            io::stdout()
                .write_all(b"out_time_us=5000000\ntotal_size=14\n")
                .unwrap();
        },
        "tree" => {
            let go = PathBuf::from(std::env::var_os("OXIDE_TEST_CHILD_GO").unwrap());
            while !go.exists() {
                thread::sleep(Duration::from_millis(10));
            }
            let descendant =
                PathBuf::from(std::env::var_os("OXIDE_TEST_CHILD_DESCENDANT").unwrap());
            let mut child = helper_command("hang", &descendant);
            // Keep inherited pipes open deliberately; the owned process tree must stop them.
            child.spawn().unwrap().wait().unwrap();
        }
        "hang" => {}
        _ => panic!("unknown fixture mode"),
    }
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn probe_accepts_version_and_rejects_failed_exit() {
    let files = TestFiles::new();
    let stop = AtomicBool::new(false);
    assert_eq!(
        probe_ffmpeg(
            helper_command("version", &files.path("pid")),
            &stop,
            Duration::from_secs(3)
        )
        .unwrap(),
        "ffmpeg version fixture"
    );
    assert!(
        probe_ffmpeg(
            helper_command("fail", &files.path("pid")),
            &stop,
            Duration::from_secs(3)
        )
        .is_err()
    );
    assert_child_exited(&files.path("pid"));
}

#[test]
fn hung_probe_times_out_and_is_reaped() {
    let files = TestFiles::new();
    let started = Instant::now();
    let error = probe_ffmpeg(
        helper_command("hang", &files.path("pid")),
        &AtomicBool::new(false),
        Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_child_exited(&files.path("pid"));
}

#[test]
fn probe_shutdown_interrupts_wait_and_reaps_process() {
    let files = TestFiles::new();
    let command = helper_command("hang", &files.path("pid"));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker =
        thread::spawn(move || probe_ffmpeg(command, &worker_stop, Duration::from_secs(30)));
    wait_until(|| files.path("pid").exists());
    stop.store(true, Ordering::Release);
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    assert_child_exited(&files.path("pid"));
}

#[test]
fn probe_rejects_unbounded_output_without_newlines() {
    let files = TestFiles::new();
    let error = probe_ffmpeg(
        helper_command("flood", &files.path("pid")),
        &AtomicBool::new(false),
        Duration::from_secs(5),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_child_exited(&files.path("pid"));
}

#[test]
fn cancel_stalled_conversion_reaps_child_and_removes_partial_output() {
    let files = TestFiles::new();
    let spec = files.spec();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    let (path_tx, path_rx) = crossbeam_channel::bounded(1);
    let pid = files.path("pid");
    let worker = thread::spawn(move || {
        run_job_with(spec, &worker_cancel, &tx, |working| {
            path_tx.send(working.to_owned()).unwrap();
            let mut command = helper_command("hang", &pid);
            command.env("OXIDE_TEST_CHILD_OUTPUT", working);
            command
        })
    });
    let working = path_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_until(|| working.exists());
    cancel.store(true, Ordering::Release);
    worker.join().unwrap();
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, ConverterEvent::Canceled { .. }))
    );
    assert!(!working.exists());
    assert!(!files.path("result.mp4").exists());
    assert_child_exited(&files.path("pid"));
}

#[test]
fn dropping_active_job_cancels_and_joins_task() {
    let files = TestFiles::new();
    let spec = files.spec();
    let output = spec.output.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    let pid = files.path("pid");
    let worker = thread::spawn(move || {
        run_job_with(spec, &worker_cancel, &tx, |path| {
            let mut command = helper_command("hang", &pid);
            command.env("OXIDE_TEST_CHILD_OUTPUT", path);
            command
        })
    });
    let job = ActiveJob {
        output,
        cancel,
        worker: Some(worker),
    };
    wait_until(|| files.path("pid").exists());
    // This is the same ownership path used by Quit, disconnect and dispatcher unwind.
    drop(job);
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, ConverterEvent::Canceled { .. }))
    );
    assert_child_exited(&files.path("pid"));
    assert_eq!(fs::read_dir(&files.0).unwrap().count(), 2); // source and PID only
}

#[test]
fn failed_conversion_keeps_final_diagnostic_and_cleans_output() {
    let files = TestFiles::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut working = PathBuf::new();
    run_job_with(files.spec(), &AtomicBool::new(false), &tx, |path| {
        working = path.to_owned();
        let mut command = helper_command("fail", &files.path("pid"));
        command.env("OXIDE_TEST_CHILD_OUTPUT", path);
        command
    });
    assert!(rx.try_iter().any(|event| matches!(event, ConverterEvent::Failed { message, .. } if message.contains("fixture encoder failed without newline"))));
    assert!(!working.exists());
    assert_child_exited(&files.path("pid"));
}

#[test]
fn cancellation_reaps_encoder_when_event_queue_is_full_before_or_after_started() {
    for fill_before_started in [true, false] {
        let files = TestFiles::new();
        let spec = files.spec();
        let (tx, rx) = crossbeam_channel::bounded(1);
        if fill_before_started {
            tx.send(ConverterEvent::QueueUpdated { pending: 0 })
                .unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (path_tx, path_rx) = crossbeam_channel::bounded(1);
        let pid = files.path("pid");
        let worker = thread::spawn(move || {
            run_job_with(spec, &worker_cancel, &tx, |path| {
                path_tx.send(path.to_owned()).unwrap();
                let mut command = helper_command("progress-flood", &pid);
                command.env("OXIDE_TEST_CHILD_OUTPUT", path);
                command
            });
        });
        let working = path_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        wait_until(|| files.path("pid").exists() && working.exists() && rx.is_full());
        let started = Instant::now();
        cancel.store(true, Ordering::Release);
        // Do not drain the UI queue. Cancellation must stop the native process
        // even when delivering Started or progress has no available capacity.
        assert_child_exited(&files.path("pid"));
        wait_until(|| !working.exists());
        assert!(started.elapsed() < Duration::from_secs(3));
        eprintln!(
            "saturated converter: before_started={fill_before_started}, encoder stopped in {:?}",
            started.elapsed()
        );
        drop(rx); // releases the terminal notification; not required to kill child
        worker.join().unwrap();
        assert!(!working.parent().unwrap().exists());
        assert!(!files.path("result.mp4").exists());
    }
}

#[test]
fn successful_conversion_commits_result_and_reports_completion() {
    let files = TestFiles::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    run_job_with(files.spec(), &AtomicBool::new(false), &tx, |path| {
        let mut command = helper_command("success", &files.path("pid"));
        command.env("OXIDE_TEST_CHILD_OUTPUT", path);
        command
    });
    assert_eq!(
        fs::read(files.path("result.mp4")).unwrap(),
        b"partial output"
    );
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, ConverterEvent::Completed { .. }))
    );
    assert_child_exited(&files.path("pid"));
}

#[test]
fn worker_panic_is_terminal_and_raii_cleans_process_and_temporary_file() {
    let files = TestFiles::new();
    let output = files.path("result.mp4");
    let working = WorkingOutput::new(&output).unwrap();
    let owned_path = working.path.clone();
    let pid = files.path("pid");
    let worker = thread::spawn(move || {
        let _output = working;
        fs::write(&_output.path, b"partial").unwrap();
        let _child = ManagedChild::spawn(helper_command("hang", &pid)).unwrap();
        wait_until(|| pid.exists());
        panic!("job failure");
    });
    let job = ActiveJob {
        output,
        cancel: Arc::new(AtomicBool::new(false)),
        worker: Some(worker),
    };
    wait_until(|| job.is_finished());
    let (tx, rx) = crossbeam_channel::unbounded();
    job.finish(&tx);
    assert!(matches!(
        rx.try_recv().unwrap(),
        ConverterEvent::Failed { .. }
    ));
    assert!(!owned_path.exists());
    assert_child_exited(&files.path("pid"));
}

#[cfg(windows)]
#[test]
fn dropping_owner_stops_descendant_that_keeps_pipes_open() {
    let files = TestFiles::new();
    let mut command = helper_command("tree", &files.path("pid"));
    command
        .env("OXIDE_TEST_CHILD_GO", files.path("go"))
        .env("OXIDE_TEST_CHILD_DESCENDANT", files.path("descendant"));
    let child = ManagedChild::spawn(command).unwrap();
    fs::write(files.path("go"), b"go").unwrap();
    wait_until(|| files.path("descendant").exists());
    drop(child);
    assert_child_exited(&files.path("pid"));
    assert_child_exited(&files.path("descendant"));
}

#[test]
fn destination_created_after_validation_is_not_replaced_or_reported_as_completed() {
    let files = TestFiles::new();
    let spec = files.spec();
    let output = spec.output.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut staging = PathBuf::new();
    run_job_with(spec, &AtomicBool::new(false), &tx, |path| {
        // validate_job already succeeded. Reproduce the exact race before the
        // encoder finishes, with no timing-dependent sleeps or real FFmpeg.
        fs::write(&output, b"created by another application").unwrap();
        staging = path.parent().unwrap().to_owned();
        let mut command = helper_command("success", &files.path("pid"));
        command.env("OXIDE_TEST_CHILD_OUTPUT", path);
        command
    });
    assert_eq!(fs::read(output).unwrap(), b"created by another application");
    assert!(!staging.exists());
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|event| matches!(event,
        ConverterEvent::Failed { message, .. } if message.contains("Он не изменён"))));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ConverterEvent::Completed { .. }))
    );
    assert_child_exited(&files.path("pid"));
}

#[test]
fn late_destination_alias_cannot_replace_the_source_file() {
    let files = TestFiles::new();
    let spec = files.spec();
    let source = spec.source.clone();
    let output = spec.output.clone();
    let (tx, rx) = crossbeam_channel::unbounded();
    run_job_with(spec, &AtomicBool::new(false), &tx, |path| {
        fs::hard_link(&source, &output).unwrap();
        let mut command = helper_command("success", &files.path("pid"));
        command.env("OXIDE_TEST_CHILD_OUTPUT", path);
        command
    });
    assert_eq!(fs::read(source).unwrap(), b"test source");
    assert_eq!(fs::read(output).unwrap(), b"test source");
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, ConverterEvent::Failed { .. }))
    );
}

#[test]
fn two_publishers_cannot_overwrite_each_others_result() {
    let files = TestFiles::new();
    let output = files.path("результат с пробелами.mp4");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [b"first".as_slice(), b"second".as_slice()]
        .into_iter()
        .map(|bytes| {
            let output = output.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let staging = WorkingOutput::new(&output).unwrap();
                fs::write(&staging.path, bytes).unwrap();
                barrier.wait();
                (bytes, publish_output(&staging.path, &output))
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap().0;
    assert_eq!(fs::read(&output).unwrap(), winner);
    let failure = results
        .iter()
        .find_map(|(_, result)| result.as_ref().err())
        .unwrap();
    assert_eq!(failure.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_dir(&files.0).unwrap().count(), 1);
}

#[test]
fn staging_preserves_extension_is_unique_and_cleans_up_on_spawn_failure() {
    let files = TestFiles::new();
    let spec = files.spec();
    {
        let first = WorkingOutput::new(&spec.output).unwrap();
        let second = WorkingOutput::new(&spec.output).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(first.path.extension(), spec.output.extension());
        assert!(!first.path.exists()); // FFmpeg -n can create the file itself.
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    run_job_with(spec, &AtomicBool::new(false), &tx, |_| {
        converter_process(&files.path("encoder-does-not-exist.exe"))
    });
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, ConverterEvent::Failed { .. }))
    );
    assert_eq!(fs::read_dir(&files.0).unwrap().count(), 1); // source only
}

#[cfg(windows)]
#[test]
fn publication_failure_keeps_destination_and_cleans_staging() {
    use std::os::windows::fs::OpenOptionsExt;
    let files = TestFiles::new();
    let output = files.path("result.mp4");
    fs::write(&output, b"existing locked file").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&output)
        .unwrap();
    let staging_directory;
    {
        let staging = WorkingOutput::new(&output).unwrap();
        staging_directory = staging.path.parent().unwrap().to_owned();
        fs::write(&staging.path, b"converted").unwrap();
        assert!(publish_output(&staging.path, &output).is_err());
    }
    drop(lock);
    assert_eq!(fs::read(output).unwrap(), b"existing locked file");
    assert!(!staging_directory.exists());
}
