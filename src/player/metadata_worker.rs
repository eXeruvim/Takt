//! Single cancellable worker for tag and artwork reads from local media.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use crate::player::{
    audio_metadata::AudioMetadata,
    latest_worker::{LatestWorker, WakeCallback},
};

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

pub(super) struct MetadataWorker {
    worker: LatestWorker<MetadataRequest, MetadataResult>,
}

impl MetadataWorker {
    pub(super) fn new(wake: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Self> {
        Self::with_loader(Arc::new(wake), Arc::new(AudioMetadata::read_cancellable))
    }

    fn with_loader(wake: WakeCallback, loader: MetadataLoader) -> anyhow::Result<Self> {
        Ok(Self {
            worker: LatestWorker::new(
                "oxide-metadata",
                "Не удалось запустить поток чтения метаданных",
                "Поток чтения метаданных завершился с паникой",
                wake,
                Arc::new(move |request: MetadataRequest, cancelled| {
                    loader(&request.path, cancelled).map(|metadata| MetadataResult {
                        generation: request.generation,
                        path: request.path,
                        metadata,
                    })
                }),
            )?,
        })
    }

    pub(super) fn request(&self, generation: u64, path: PathBuf) {
        self.worker.submit(MetadataRequest { generation, path });
    }

    pub(super) fn cancel(&self) {
        self.worker.cancel();
    }

    pub(super) fn take_result(&self) -> Option<MetadataResult> {
        self.worker.take_result()
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
