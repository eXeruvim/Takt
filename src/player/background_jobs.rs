//! Typed ownership and relevance filtering for player background work.

use std::path::{Path, PathBuf};

use crate::player::{
    metadata_worker::{MetadataResult, MetadataWorker},
    playlist::DirectoryPlaylist,
    playlist_worker::{PlaylistRequest, PlaylistWorker},
};

pub(super) enum ReadyPlaylist {
    FileExpansion(anyhow::Result<DirectoryPlaylist>),
    Directory {
        result: anyhow::Result<DirectoryPlaylist>,
        elapsed: std::time::Duration,
    },
}

#[derive(Default)]
pub(super) struct ReadyBackground {
    pub(super) metadata: Option<MetadataResult>,
    pub(super) playlist: Option<ReadyPlaylist>,
}

pub(super) struct BackgroundJobs {
    metadata: MetadataWorker,
    playlist: PlaylistWorker,
}

impl BackgroundJobs {
    pub(super) fn new(wake: impl Fn() + Clone + Send + Sync + 'static) -> anyhow::Result<Self> {
        Ok(Self {
            metadata: MetadataWorker::new(wake.clone())?,
            playlist: PlaylistWorker::new(wake)?,
        })
    }

    pub(super) fn request_metadata(&self, generation: u64, path: PathBuf) {
        self.metadata.request(generation, path);
    }

    pub(super) fn expand_file(&self, generation: u64, path: PathBuf) {
        self.playlist.expand_file(generation, path);
    }

    pub(super) fn open_directory(&self, path: PathBuf) {
        self.playlist.open_directory(path);
    }

    pub(super) fn cancel_metadata(&self) {
        self.metadata.cancel();
    }

    pub(super) fn cancel_playlist(&self) {
        self.playlist.cancel();
    }

    pub(super) fn cancel_all(&self) {
        self.metadata.cancel();
        self.playlist.cancel();
    }

    pub(super) fn take_ready(
        &self,
        current_generation: u64,
        current_path: Option<&Path>,
    ) -> ReadyBackground {
        let playlist = self
            .playlist
            .take_result()
            .and_then(|result| match result.request {
                PlaylistRequest::FileExpansion { generation, path } => {
                    is_current(generation, &path, current_generation, current_path)
                        .then_some(ReadyPlaylist::FileExpansion(result.playlist))
                }
                PlaylistRequest::Directory(_) => Some(ReadyPlaylist::Directory {
                    result: result.playlist,
                    elapsed: result.elapsed,
                }),
            });

        // A completed directory request replaces the current media in this
        // same dispatch cycle. Do not briefly publish metadata belonging to
        // the item that is about to be replaced.
        let replaces_media = matches!(playlist, Some(ReadyPlaylist::Directory { .. }));
        let metadata = self.metadata.take_result().and_then(|result| {
            metadata_is_current(
                replaces_media,
                result.generation,
                &result.path,
                current_generation,
                current_path,
            )
            .then_some(result)
        });

        ReadyBackground { metadata, playlist }
    }
}

fn metadata_is_current(
    replaces_media: bool,
    generation: u64,
    path: &Path,
    current_generation: u64,
    current_path: Option<&Path>,
) -> bool {
    !replaces_media && is_current(generation, path, current_generation, current_path)
}

fn is_current(
    generation: u64,
    path: &Path,
    current_generation: u64,
    current_path: Option<&Path>,
) -> bool {
    generation == current_generation && current_path == Some(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{is_current, metadata_is_current};

    #[test]
    fn background_result_requires_matching_generation_and_path() {
        let current = Path::new("current.mp3");
        assert!(is_current(7, current, 7, Some(current)));
        assert!(!is_current(6, current, 7, Some(current)));
        assert!(!is_current(7, Path::new("previous.mp3"), 7, Some(current)));
        assert!(!is_current(7, current, 7, None));
    }

    #[test]
    fn replacing_directory_result_suppresses_metadata_from_previous_media() {
        let current = Path::new("current.mp3");
        assert!(metadata_is_current(false, 7, current, 7, Some(current)));
        assert!(!metadata_is_current(true, 7, current, 7, Some(current)));
    }
}
