//! Explicit state for resume prompts and position persistence across media changes.

use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub(super) struct PlaybackState {
    resume: ResumeState,
    position_recording_suppressed_for: Option<PathBuf>,
    generation: u64,
    expected_playlist_entry_id: Option<i64>,
    awaiting_start: bool,
    active_playlist_entry_id: Option<i64>,
}

#[derive(Debug, Default)]
enum ResumeState {
    #[default]
    Inactive,
    Loading(PathBuf),
    AwaitingChoice {
        path: PathBuf,
        position: f64,
    },
}

impl PlaybackState {
    pub(super) fn begin_media_change(
        &mut self,
        expected_path: Option<PathBuf>,
        playlist_entry_id: Option<i64>,
    ) {
        self.position_recording_suppressed_for = None;
        self.set_expected_path(expected_path, playlist_entry_id);
    }

    pub(super) fn media_change_failed(&mut self) {
        self.resume = ResumeState::Inactive;
        self.expected_playlist_entry_id = None;
        self.awaiting_start = false;
        self.active_playlist_entry_id = None;
    }

    pub(super) fn expect_resume_for(
        &mut self,
        path: Option<PathBuf>,
        playlist_entry_id: Option<i64>,
    ) {
        self.set_expected_path(path, playlist_entry_id);
    }

    pub(super) fn offer_resume(&mut self, path: PathBuf, position: f64) {
        self.resume = ResumeState::AwaitingChoice { path, position };
    }

    pub(super) fn finish_loading_without_resume(&mut self) {
        self.resume = ResumeState::Inactive;
    }

    pub(super) fn take_resume_choice(&mut self) -> Option<(PathBuf, f64)> {
        match std::mem::take(&mut self.resume) {
            ResumeState::AwaitingChoice { path, position } => Some((path, position)),
            state => {
                self.resume = state;
                None
            }
        }
    }

    pub(super) fn suppress_position_recording_for(&mut self, path: Option<PathBuf>) {
        self.resume = ResumeState::Inactive;
        self.position_recording_suppressed_for = path;
    }

    pub(super) fn cancel_resume(&mut self) {
        self.resume = ResumeState::Inactive;
    }

    pub(super) fn file_loaded(&mut self, path: &Path) {
        if self.position_recording_suppressed_for.as_deref() != Some(path) {
            self.position_recording_suppressed_for = None;
        }
    }

    pub(super) fn should_record_media(&self, path: &Path) -> bool {
        self.position_recording_suppressed_for.as_deref() != Some(path)
    }

    pub(super) fn should_record_position(&self, path: &Path) -> bool {
        let resume_blocks_path = match &self.resume {
            ResumeState::Loading(expected) => expected == path,
            ResumeState::AwaitingChoice { path: pending, .. } => pending == path,
            ResumeState::Inactive => false,
        };
        !resume_blocks_path && self.should_record_media(path)
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn start_file(&mut self, playlist_entry_id: i64) -> bool {
        if self.awaiting_start {
            if self
                .expected_playlist_entry_id
                .is_some_and(|expected| expected != playlist_entry_id)
            {
                return false;
            }
            self.expected_playlist_entry_id = None;
            self.awaiting_start = false;
            self.active_playlist_entry_id = Some(playlist_entry_id);
            return true;
        }

        self.active_playlist_entry_id == Some(playlist_entry_id)
    }

    pub(super) fn end_file(&mut self, playlist_entry_id: i64) -> bool {
        if self.active_playlist_entry_id != Some(playlist_entry_id) {
            return false;
        }
        self.active_playlist_entry_id = None;
        true
    }

    pub(super) fn has_active_media(&self) -> bool {
        self.active_playlist_entry_id.is_some()
    }

    fn set_expected_path(&mut self, path: Option<PathBuf>, playlist_entry_id: Option<i64>) {
        if path.is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.expected_playlist_entry_id = playlist_entry_id;
            self.awaiting_start = true;
            self.active_playlist_entry_id = None;
        }
        self.resume = path
            .map(ResumeState::Loading)
            .unwrap_or(ResumeState::Inactive);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::PlaybackState;

    #[test]
    fn loading_and_prompt_block_position_until_resume_is_resolved() {
        let path = PathBuf::from("video.mp4");
        let mut state = PlaybackState::default();

        state.begin_media_change(Some(path.clone()), Some(1));
        assert!(!state.should_record_position(&path));

        state.offer_resume(path.clone(), 42.5);
        assert!(!state.should_record_position(&path));
        assert_eq!(state.take_resume_choice(), Some((path.clone(), 42.5)));
        assert!(state.should_record_position(&path));
    }

    #[test]
    fn unrelated_resume_command_does_not_cancel_loading_guard() {
        let path = PathBuf::from("audio.flac");
        let mut state = PlaybackState::default();

        state.expect_resume_for(Some(path.clone()), Some(1));
        assert_eq!(state.take_resume_choice(), None);
        assert!(!state.should_record_position(&path));

        state.finish_loading_without_resume();
        assert!(state.should_record_position(&path));
    }

    #[test]
    fn clearing_history_suppresses_only_the_current_file() {
        let current = PathBuf::from("current.mp3");
        let next = PathBuf::from("next.mp3");
        let mut state = PlaybackState::default();

        state.suppress_position_recording_for(Some(current.clone()));
        assert!(!state.should_record_media(&current));
        assert!(!state.should_record_position(&current));

        state.file_loaded(&next);
        assert!(state.should_record_media(&next));
        assert!(state.should_record_position(&next));
    }

    #[test]
    fn failed_media_change_cancels_resume_but_preserves_clear_suppression() {
        let current = PathBuf::from("current.mp3");
        let next = PathBuf::from("missing.mp3");
        let mut state = PlaybackState::default();

        state.suppress_position_recording_for(Some(current.clone()));
        state.expect_resume_for(Some(next.clone()), Some(2));
        state.media_change_failed();

        assert!(!state.should_record_position(&current));
        assert!(state.should_record_position(&next));
    }

    #[test]
    fn generation_changes_only_when_a_file_load_is_expected() {
        let mut state = PlaybackState::default();
        assert_eq!(state.generation(), 0);

        state.begin_media_change(None, None);
        assert_eq!(state.generation(), 0);

        state.begin_media_change(Some(PathBuf::from("first.mp3")), Some(1));
        let first = state.generation();
        assert_eq!(state.generation(), first);

        state.expect_resume_for(Some(PathBuf::from("second.mp3")), Some(2));
        assert_ne!(state.generation(), first);
        assert_eq!(state.generation(), first + 1);
    }

    #[test]
    fn only_latest_pending_start_becomes_active() {
        let mut state = PlaybackState::default();
        state.begin_media_change(Some(PathBuf::from("first.mp4")), Some(10));
        state.begin_media_change(Some(PathBuf::from("second.mp4")), Some(11));

        assert!(!state.start_file(10));
        assert!(!state.has_active_media());
        assert!(state.start_file(11));
        assert!(state.has_active_media());
        assert!(!state.end_file(10));
        assert!(state.has_active_media());
        assert!(state.end_file(11));
        assert!(!state.has_active_media());
    }

    #[test]
    fn repeated_start_for_active_loop_is_accepted_without_pending_load() {
        let mut state = PlaybackState::default();
        state.begin_media_change(Some(PathBuf::from("loop.mp3")), Some(42));
        assert!(state.start_file(42));
        assert!(state.start_file(42));
        assert!(!state.start_file(43));
    }
}
