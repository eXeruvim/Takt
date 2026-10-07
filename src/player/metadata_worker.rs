//! Single cancellable worker for tag and artwork reads from local media.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use anyhow::Context;

use crate::player::audio_metadata::AudioMetadata;

type WakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;
type MetadataLoader =
    Arc<dyn Fn(&Path, Arc<AtomicBool>) -> Option<AudioMetadata> + Send + Sync + 'static>;

struct MetadataRequest {
    generation: u64,
    path: PathBuf,
}

pub(super) struct MetadataResult {
    pub(super) generation: u64,
    pub(super) path: PathBuf,
    pub(super) metadata: AudioMetadata,
}

#[derive(Default)]
struct WorkerState {
    pending: Option<MetadataRequest>,
    completed: Option<MetadataResult>,
    active_cancel: Option<Arc<AtomicBool>>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<WorkerState>,
    ready: Condvar,
}

pub(super) struct MetadataWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl MetadataWorker {
    pub(super) fn new(wake: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Self> {
        Self::with_loader(Arc::new(wake), Arc::new(AudioMetadata::read_cancellable))
    }

    fn with_loader(wake: WakeCallback, loader: MetadataLoader) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("oxide-metadata".to_owned())
            .spawn(move || run_worker(worker_shared, wake, loader))
            .context("Не удалось запустить поток чтения метаданных")?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub(super) fn request(&self, generation: u64, path: PathBuf) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.shutdown {
            return;
        }
        if let Some(cancelled) = state.active_cancel.as_ref() {
            cancelled.store(true, Ordering::Release);
        }
        state.pending = Some(MetadataRequest { generation, path });
        state.completed = None;
        self.shared.ready.notify_one();
    }

    pub(super) fn cancel(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(cancelled) = state.active_cancel.as_ref() {
            cancelled.store(true, Ordering::Release);
        }
        state.pending = None;
        state.completed = None;
    }

    pub(super) fn take_result(&self) -> Option<MetadataResult> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .completed
            .take()
    }
}

impl Drop for MetadataWorker {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.shutdown = true;
            state.pending = None;
            state.completed = None;
            if let Some(cancelled) = state.active_cancel.as_ref() {
                cancelled.store(true, Ordering::Release);
            }
            self.shared.ready.notify_one();
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("Поток чтения метаданных завершился с паникой");
        }
    }
}

fn run_worker(shared: Arc<Shared>, wake: WakeCallback, loader: MetadataLoader) {
    loop {
        let (request, cancelled) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            while state.pending.is_none() && !state.shutdown {
                state = shared
                    .ready
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            if state.shutdown {
                return;
            }
            let request = state.pending.take().expect("pending request checked");
            let cancelled = Arc::new(AtomicBool::new(false));
            state.active_cancel = Some(Arc::clone(&cancelled));
            (request, cancelled)
        };

        let metadata = loader(&request.path, Arc::clone(&cancelled));
        let should_wake = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state
                .active_cancel
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &cancelled))
            {
                state.active_cancel = None;
            }
            if state.shutdown {
                return;
            }
            if cancelled.load(Ordering::Acquire) || state.pending.is_some() {
                false
            } else if let Some(metadata) = metadata {
                state.completed = Some(MetadataResult {
                    generation: request.generation,
                    path: request.path,
                    metadata,
                });
                true
            } else {
                false
            }
        };
        if should_wake {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use super::{AudioMetadata, MetadataWorker};

    #[test]
    fn newer_request_cancels_and_replaces_an_active_read() {
        let (started_tx, started_rx) = mpsc::channel();
        let worker = MetadataWorker::with_loader(
            Arc::new(|| {}),
            Arc::new(move |path, cancelled: Arc<AtomicBool>| {
                if path.ends_with("slow.mp3") {
                    let _ = started_tx.send(());
                    while !cancelled.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    return None;
                }
                Some(AudioMetadata {
                    title: path.to_string_lossy().into_owned(),
                    ..AudioMetadata::default()
                })
            }),
        )
        .unwrap();

        worker.request(1, "slow.mp3".into());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.request(2, "latest.mp3".into());

        let deadline = Instant::now() + Duration::from_secs(1);
        let result = loop {
            if let Some(result) = worker.take_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "latest metadata did not finish");
            std::thread::yield_now();
        };
        assert_eq!(result.generation, 2);
        assert_eq!(result.path, std::path::PathBuf::from("latest.mp3"));
        assert_eq!(result.metadata.title, "latest.mp3");
    }

    #[test]
    fn dropping_worker_cancels_and_joins_an_active_read() {
        let (started_tx, started_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_in_loader = Arc::clone(&finished);
        let worker = MetadataWorker::with_loader(
            Arc::new(|| {}),
            Arc::new(move |_, cancelled: Arc<AtomicBool>| {
                let _ = started_tx.send(());
                while !cancelled.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                finished_in_loader.store(true, Ordering::Release);
                None
            }),
        )
        .unwrap();

        worker.request(1, "slow.mp3".into());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(worker);
        assert!(finished.load(Ordering::Acquire));
    }
}
