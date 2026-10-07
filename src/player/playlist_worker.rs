//! Single cancellable worker for bounded directory playlist discovery.

use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use anyhow::Context;

use crate::player::playlist::DirectoryPlaylist;

type WakeCallback = Arc<dyn Fn() + Send + Sync + 'static>;
type PlaylistLoader = Arc<
    dyn Fn(&PlaylistRequest, Arc<AtomicBool>) -> anyhow::Result<Option<DirectoryPlaylist>>
        + Send
        + Sync
        + 'static,
>;

pub(super) enum PlaylistRequest {
    FileExpansion { generation: u64, path: PathBuf },
    Directory(PathBuf),
}

pub(super) struct PlaylistResult {
    pub(super) request: PlaylistRequest,
    pub(super) playlist: anyhow::Result<DirectoryPlaylist>,
}

#[derive(Default)]
struct WorkerState {
    pending: Option<PlaylistRequest>,
    completed: Option<PlaylistResult>,
    active_cancel: Option<Arc<AtomicBool>>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<WorkerState>,
    ready: Condvar,
}

pub(super) struct PlaylistWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PlaylistWorker {
    pub(super) fn new(wake: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Self> {
        Self::with_loader(
            Arc::new(wake),
            Arc::new(|request, cancelled| match request {
                PlaylistRequest::FileExpansion { path, .. } => {
                    DirectoryPlaylist::load_from_path_cancellable(path, &cancelled)
                }
                PlaylistRequest::Directory(path) => {
                    DirectoryPlaylist::load_from_directory_cancellable(path, &cancelled)
                }
            }),
        )
    }

    fn with_loader(wake: WakeCallback, loader: PlaylistLoader) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("oxide-playlist".to_owned())
            .spawn(move || run_worker(worker_shared, wake, loader))
            .context("Не удалось запустить поток формирования очереди")?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub(super) fn expand_file(&self, generation: u64, path: PathBuf) {
        self.submit(PlaylistRequest::FileExpansion { generation, path });
    }

    pub(super) fn open_directory(&self, path: PathBuf) {
        self.submit(PlaylistRequest::Directory(path));
    }

    pub(super) fn cancel(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancel_state(&mut state);
    }

    pub(super) fn take_result(&self) -> Option<PlaylistResult> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .completed
            .take()
    }

    fn submit(&self, request: PlaylistRequest) {
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
        state.pending = Some(request);
        state.completed = None;
        self.shared.ready.notify_one();
    }
}

impl Drop for PlaylistWorker {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.shutdown = true;
            cancel_state(&mut state);
            self.shared.ready.notify_one();
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("Поток формирования очереди завершился с паникой");
        }
    }
}

fn cancel_state(state: &mut WorkerState) {
    if let Some(cancelled) = state.active_cancel.as_ref() {
        cancelled.store(true, Ordering::Release);
    }
    state.pending = None;
    state.completed = None;
}

fn run_worker(shared: Arc<Shared>, wake: WakeCallback, loader: PlaylistLoader) {
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

        let result = loader(&request, Arc::clone(&cancelled));
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
            } else {
                match result {
                    Ok(Some(playlist)) => {
                        state.completed = Some(PlaylistResult {
                            request,
                            playlist: Ok(playlist),
                        });
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        state.completed = Some(PlaylistResult {
                            request,
                            playlist: Err(error),
                        });
                        true
                    }
                }
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
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use super::{PlaylistRequest, PlaylistWorker};

    fn request_path(request: &PlaylistRequest) -> &Path {
        match request {
            PlaylistRequest::FileExpansion { path, .. } | PlaylistRequest::Directory(path) => path,
        }
    }

    #[test]
    fn newer_scan_cancels_and_replaces_an_active_scan() {
        let (started_tx, started_rx) = mpsc::channel();
        let worker = PlaylistWorker::with_loader(
            Arc::new(|| {}),
            Arc::new(move |request, cancelled: Arc<AtomicBool>| {
                if request_path(request).ends_with("slow") {
                    let _ = started_tx.send(());
                    while !cancelled.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    return Ok(None);
                }
                Err(anyhow::anyhow!("latest scan"))
            }),
        )
        .unwrap();

        worker.open_directory("slow".into());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.expand_file(2, "latest.mp3".into());

        let deadline = Instant::now() + Duration::from_secs(1);
        let result = loop {
            if let Some(result) = worker.take_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "latest scan did not finish");
            std::thread::yield_now();
        };
        assert_eq!(result.playlist.unwrap_err().to_string(), "latest scan");
    }

    #[test]
    fn dropping_worker_cancels_and_joins_an_active_scan() {
        let (started_tx, started_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_in_loader = Arc::clone(&finished);
        let worker = PlaylistWorker::with_loader(
            Arc::new(|| {}),
            Arc::new(move |_, cancelled: Arc<AtomicBool>| {
                let _ = started_tx.send(());
                while !cancelled.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                finished_in_loader.store(true, Ordering::Release);
                Ok(None)
            }),
        )
        .unwrap();

        worker.open_directory("slow".into());
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(worker);
        assert!(finished.load(Ordering::Acquire));
    }
}
