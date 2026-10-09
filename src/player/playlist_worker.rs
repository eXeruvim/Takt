//! Single cancellable worker for bounded directory playlist discovery.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use crate::player::{
    latest_worker::{LatestWorker, WakeCallback},
    playlist::DirectoryPlaylist,
};

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
    pub(super) elapsed: Duration,
}

pub(super) struct PlaylistWorker {
    worker: LatestWorker<PlaylistRequest, PlaylistResult>,
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
        Ok(Self {
            worker: LatestWorker::new(
                "oxide-playlist",
                "Не удалось запустить поток формирования очереди",
                "Поток формирования очереди завершился с паникой",
                wake,
                Arc::new(move |request: PlaylistRequest, cancelled| {
                    let started = Instant::now();
                    let loaded = loader(&request, cancelled);
                    let elapsed = started.elapsed();
                    match loaded {
                        Ok(Some(playlist)) => Some(PlaylistResult {
                            request,
                            playlist: Ok(playlist),
                            elapsed,
                        }),
                        Ok(None) => None,
                        Err(error) => Some(PlaylistResult {
                            request,
                            playlist: Err(error),
                            elapsed,
                        }),
                    }
                }),
            )?,
        })
    }

    pub(super) fn expand_file(&self, generation: u64, path: PathBuf) {
        self.submit(PlaylistRequest::FileExpansion { generation, path });
    }

    pub(super) fn open_directory(&self, path: PathBuf) {
        self.submit(PlaylistRequest::Directory(path));
    }

    pub(super) fn cancel(&self) {
        self.worker.cancel();
    }

    pub(super) fn take_result(&self) -> Option<PlaylistResult> {
        self.worker.take_result()
    }

    fn submit(&self, request: PlaylistRequest) {
        self.worker.submit(request);
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
