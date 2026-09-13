use std::{
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) fn finish_workers(
    mut workers: Vec<(&str, JoinHandle<()>)>,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut failed = Vec::new();
    while !workers.is_empty() {
        let mut index = 0;
        while index < workers.len() {
            if workers[index].1.is_finished() {
                let (name, worker) = workers.swap_remove(index);
                if worker.join().is_err() {
                    failed.push(format!("{name}: panic"));
                }
            } else {
                index += 1;
            }
        }
        if !workers.is_empty() {
            if Instant::now() >= deadline {
                failed.extend(
                    workers
                        .iter()
                        .map(|(name, _)| format!("{name}: shutdown timeout")),
                );
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    anyhow::ensure!(
        failed.is_empty(),
        "Background shutdown failed: {}",
        failed.join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_worker_does_not_block_other_workers_or_shutdown_deadline() {
        let (release, wait) = std::sync::mpsc::channel();
        let blocked = thread::spawn(move || {
            let _ = wait.recv();
        });
        let done = thread::spawn(|| {});
        let started = Instant::now();
        let result = finish_workers(
            vec![("blocked", blocked), ("done", done)],
            Duration::from_millis(40),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("blocked: shutdown timeout")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = release.send(());
    }

    #[test]
    fn worker_panic_is_reported() {
        let worker = thread::spawn(|| panic!("test worker"));
        assert!(
            finish_workers(vec![("worker", worker)], Duration::from_secs(2))
                .unwrap_err()
                .to_string()
                .contains("worker: panic")
        );
    }
}
