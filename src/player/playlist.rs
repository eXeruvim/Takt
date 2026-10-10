use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use crate::player::limits::{PATH_TEXT_BYTES, UI_TEXT_BYTES, bounded_text};

const MAX_PLAYLIST_FILES: usize = 5_000;
const MAX_PLAYLIST_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_PLAYLIST_LABEL_BYTES: usize = 4 * 1024 * 1024;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv", "wmv", "mpg", "mpeg", "ogv",
];
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "wave", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wma", "aif", "aiff",
    "aifc", "ape", "wv", "tta", "alac", "mka",
];
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv", "wmv", "mpg", "mpeg", "ogv", "mp3",
    "flac", "wav", "wave", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wma", "aif", "aiff", "aifc",
    "ape", "wv", "tta", "alac", "mka",
];

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

pub fn is_video_file(path: &Path) -> bool {
    has_extension(path, VIDEO_EXTENSIONS)
}

pub fn is_audio_file(path: &Path) -> bool {
    has_extension(path, AUDIO_EXTENSIONS)
}

pub fn is_media_file(path: &Path) -> bool {
    is_video_file(path) || is_audio_file(path)
}

#[derive(Debug)]
pub struct DirectoryPlaylist {
    files: Vec<PathBuf>,
    current_index: usize,
    omitted_count: usize,
}

impl DirectoryPlaylist {
    pub(super) fn from_file(path: &Path) -> Result<Self> {
        if !path.is_file() {
            bail!("Файл не найден: {}", path.display());
        }
        if !is_media_file(path) {
            bail!("Неподдерживаемый формат медиа: {}", path.display());
        }
        let mut files = PlaylistAccumulator::new(PLAYLIST_LIMITS);
        if !files.push(path.to_path_buf()) {
            bail!("Путь открываемого файла превышает безопасный размер очереди");
        }
        let files = files.finish();
        Ok(Self {
            files: files.files,
            current_index: 0,
            omitted_count: files.omitted_count,
        })
    }

    pub(super) fn load_from_path_cancellable(
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<Self>> {
        if cancelled.load(AtomicOrdering::Acquire) {
            return Ok(None);
        }
        if !path.is_file() {
            bail!("Файл не найден: {}", path.display());
        }
        if !is_media_file(path) {
            bail!("Неподдерживаемый формат медиа: {}", path.display());
        }

        let directory = path
            .parent()
            .context("Не удалось определить папку медиафайла")?;
        let PlaylistFiles {
            files,
            omitted_count,
        } = match read_media_files(directory, Some(path), cancelled)? {
            Some(files) => files,
            None => return Ok(None),
        };

        let current_index = files
            .iter()
            .position(|candidate| candidate == path)
            .context("Открытый файл отсутствует в сформированном плейлисте")?;

        Ok(Some(Self {
            files,
            current_index,
            omitted_count,
        }))
    }

    pub(super) fn load_from_directory_cancellable(
        directory: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<Self>> {
        if cancelled.load(AtomicOrdering::Acquire) {
            return Ok(None);
        }
        if !directory.is_dir() {
            bail!("Папка не найдена: {}", directory.display());
        }

        let PlaylistFiles {
            files,
            omitted_count,
        } = match read_media_files(directory, None, cancelled)? {
            Some(files) => files,
            None => return Ok(None),
        };
        if files.is_empty() {
            bail!(
                "В папке нет поддерживаемых видео или аудио: {}",
                directory.display()
            );
        }

        Ok(Some(Self {
            files,
            current_index: 0,
            omitted_count,
        }))
    }

    pub fn current_path(&self) -> Option<&PathBuf> {
        self.files.get(self.current_index)
    }

    pub fn move_next(&mut self, wrap: bool) -> Option<PathBuf> {
        if self.current_index + 1 < self.files.len() {
            self.current_index += 1;
        } else if wrap && !self.files.is_empty() {
            self.current_index = 0;
        } else {
            return None;
        }

        self.current_path().cloned()
    }

    pub fn move_previous(&mut self, wrap: bool) -> Option<PathBuf> {
        if self.current_index > 0 {
            self.current_index -= 1;
        } else if wrap && !self.files.is_empty() {
            self.current_index = self.files.len() - 1;
        } else {
            return None;
        }

        self.current_path().cloned()
    }

    pub fn select(&mut self, index: usize) -> Option<PathBuf> {
        if index >= self.files.len() {
            return None;
        }

        self.current_index = index;
        self.current_path().cloned()
    }

    pub fn set_shuffled(&mut self, shuffled: bool) {
        let Some(current) = self.current_path().cloned() else {
            return;
        };

        if shuffled {
            self.files.retain(|candidate| candidate != &current);
            shuffle_paths(&mut self.files, random_seed());
            self.files.insert(0, current.clone());
        } else {
            sort_paths_naturally(&mut self.files);
        }

        self.current_index = self
            .files
            .iter()
            .position(|candidate| candidate == &current)
            .unwrap_or_default();
    }

    pub fn remove(&mut self, index: usize) -> Option<bool> {
        if index >= self.files.len() {
            return None;
        }

        let removed_current = index == self.current_index;
        self.files.remove(index);
        if self.files.is_empty() {
            self.current_index = 0;
        } else if index < self.current_index {
            self.current_index -= 1;
        } else if self.current_index >= self.files.len() {
            self.current_index = self.files.len() - 1;
        }

        Some(removed_current)
    }

    pub fn remove_current_and_select(&mut self, next: bool, wrap: bool) -> Option<PathBuf> {
        if self.files.is_empty() || self.current_index >= self.files.len() {
            return None;
        }

        let removed_index = self.current_index;
        self.files.remove(removed_index);
        if self.files.is_empty() {
            self.current_index = 0;
            return None;
        }

        if next {
            if removed_index < self.files.len() {
                self.current_index = removed_index;
            } else if wrap {
                self.current_index = 0;
            } else {
                self.current_index = self.files.len() - 1;
                return None;
            }
        } else if removed_index > 0 {
            self.current_index = removed_index - 1;
        } else if wrap {
            self.current_index = self.files.len() - 1;
        } else {
            self.current_index = 0;
            return None;
        }

        self.current_path().cloned()
    }

    pub fn items(&self) -> Vec<String> {
        playlist_labels(&self.files)
    }

    pub fn limit_notice(&self) -> Option<String> {
        (self.omitted_count > 0).then(|| {
            format!(
                "Файлов в очереди: {}. Ещё {} не добавлено, чтобы плеер работал быстро.",
                self.files.len(),
                self.omitted_count
            )
        })
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }
}

struct PlaylistFiles {
    files: Vec<PathBuf>,
    omitted_count: usize,
}

#[derive(Clone, Copy)]
struct PlaylistLimits {
    files: usize,
    path_bytes: usize,
    label_bytes: usize,
    single_path_bytes: usize,
}

const PLAYLIST_LIMITS: PlaylistLimits = PlaylistLimits {
    files: MAX_PLAYLIST_FILES,
    path_bytes: MAX_PLAYLIST_PATH_BYTES,
    label_bytes: MAX_PLAYLIST_LABEL_BYTES,
    single_path_bytes: PATH_TEXT_BYTES,
};

struct PlaylistAccumulator {
    files: Vec<PathBuf>,
    path_bytes: usize,
    label_bytes: usize,
    omitted_count: usize,
    limits: PlaylistLimits,
}

impl PlaylistAccumulator {
    fn new(limits: PlaylistLimits) -> Self {
        Self {
            files: Vec::new(),
            path_bytes: 0,
            label_bytes: 0,
            omitted_count: 0,
            limits,
        }
    }

    fn push(&mut self, path: PathBuf) -> bool {
        let path_bytes = path.as_os_str().as_encoded_bytes().len();
        let label_bytes = path
            .file_name()
            .map_or(0, |name| name.to_string_lossy().len())
            .min(UI_TEXT_BYTES);
        let fits = path_bytes <= self.limits.single_path_bytes
            && self.files.len() < self.limits.files
            && self.path_bytes.saturating_add(path_bytes) <= self.limits.path_bytes
            && self.label_bytes.saturating_add(label_bytes) <= self.limits.label_bytes;
        if !fits {
            self.omitted_count = self.omitted_count.saturating_add(1);
            return false;
        }

        self.files.push(path);
        self.path_bytes += path_bytes;
        self.label_bytes += label_bytes;
        true
    }

    fn finish(mut self) -> PlaylistFiles {
        sort_paths_naturally(&mut self.files);
        PlaylistFiles {
            files: self.files,
            omitted_count: self.omitted_count,
        }
    }
}

fn read_media_files(
    directory: &Path,
    priority: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<Option<PlaylistFiles>> {
    Ok(scan_media_files(directory, priority, cancelled)?.map(PlaylistAccumulator::finish))
}

fn scan_media_files(
    directory: &Path,
    priority: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<Option<PlaylistAccumulator>> {
    let mut files = PlaylistAccumulator::new(PLAYLIST_LIMITS);
    if let Some(priority) = priority
        && !files.push(priority.to_path_buf())
    {
        bail!("Путь открываемого файла превышает безопасный размер очереди");
    }

    for entry in fs::read_dir(directory)
        .with_context(|| format!("Не удалось прочитать папку: {}", directory.display()))?
        .flatten()
    {
        if cancelled.load(AtomicOrdering::Acquire) {
            return Ok(None);
        }
        let candidate = entry.path();
        if priority.is_some_and(|priority| candidate == priority) {
            continue;
        }
        let is_file = match entry.file_type() {
            Ok(file_type) if file_type.is_file() => true,
            Ok(file_type) if file_type.is_symlink() => candidate.is_file(),
            Ok(_) => false,
            Err(_) => candidate.is_file(),
        };
        if is_file && is_media_file(&candidate) {
            files.push(candidate);
        }
    }

    if cancelled.load(AtomicOrdering::Acquire) {
        return Ok(None);
    }
    Ok(Some(files))
}

fn playlist_labels(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| bounded_text(&name.to_string_lossy(), UI_TEXT_BYTES))
        })
        .collect()
}

fn sort_paths_naturally(files: &mut [PathBuf]) {
    files.sort_by(|left, right| {
        let left = left.file_name().unwrap_or_default().to_string_lossy();
        let right = right.file_name().unwrap_or_default().to_string_lossy();
        natural_cmp(&left, &right)
    });
}

#[cfg(feature = "benchmark-tools")]
#[allow(dead_code)]
pub(crate) fn benchmark_scan_media_files(directory: &Path) -> Result<(Vec<PathBuf>, usize)> {
    let cancelled = AtomicBool::new(false);
    let files = scan_media_files(directory, None, &cancelled)?
        .context("Benchmark scan was unexpectedly cancelled")?;
    Ok((files.files, files.omitted_count))
}

#[cfg(feature = "benchmark-tools")]
#[allow(dead_code)]
pub(crate) fn benchmark_sort_paths(files: &mut [PathBuf]) {
    sort_paths_naturally(files);
}

#[cfg(feature = "benchmark-tools")]
#[allow(dead_code)]
pub(crate) fn benchmark_playlist_labels(files: &[PathBuf]) -> Vec<String> {
    playlist_labels(files)
}

fn random_seed() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    time ^ u64::from(std::process::id())
}

fn shuffle_paths(files: &mut [PathBuf], mut state: u64) {
    if state == 0 {
        state = 0x9e37_79b9_7f4a_7c15;
    }

    for index in (1..files.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let other = state as usize % (index + 1);
        files.swap(index, other);
    }
}

fn natural_cmp(left: &str, right: &str) -> Ordering {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    natural_cmp_folded(&left, &right)
}

fn natural_cmp_folded(left: &str, right: &str) -> Ordering {
    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();
    let mut left_index = 0;
    let mut right_index = 0;

    while left_index < left_bytes.len() && right_index < right_bytes.len() {
        if left_bytes[left_index].is_ascii_digit() && right_bytes[right_index].is_ascii_digit() {
            let left_end = digit_run_end(left_bytes, left_index);
            let right_end = digit_run_end(right_bytes, right_index);
            let ordering = compare_digit_runs(
                &left_bytes[left_index..left_end],
                &right_bytes[right_index..right_end],
            );
            if ordering != Ordering::Equal {
                return ordering;
            }
            left_index = left_end;
            right_index = right_end;
            continue;
        }

        let left_char = left[left_index..]
            .chars()
            .next()
            .expect("index remains on a UTF-8 boundary");
        let right_char = right[right_index..]
            .chars()
            .next()
            .expect("index remains on a UTF-8 boundary");
        let ordering = left_char.cmp(&right_char);
        if ordering != Ordering::Equal {
            return ordering;
        }
        left_index += left_char.len_utf8();
        right_index += right_char.len_utf8();
    }

    left_bytes.len().cmp(&right_bytes.len())
}

fn digit_run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    end
}

fn compare_digit_runs(left: &[u8], right: &[u8]) -> Ordering {
    let left_trimmed = left.iter().position(|byte| *byte != b'0');
    let right_trimmed = right.iter().position(|byte| *byte != b'0');
    let left_significant = if let Some(index) = left_trimmed {
        &left[index..]
    } else {
        b"0"
    };
    let right_significant = if let Some(index) = right_trimmed {
        &right[index..]
    } else {
        b"0"
    };

    left_significant
        .len()
        .cmp(&right_significant.len())
        .then_with(|| left_significant.cmp(right_significant))
        .then_with(|| left.len().cmp(&right.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_video_extensions_case_insensitively() {
        assert!(is_video_file(Path::new("movie.MKV")));
        assert!(is_video_file(Path::new("clip.mp4")));
        assert!(!is_video_file(Path::new("notes.txt")));
        assert!(!is_video_file(Path::new("no-extension")));
    }

    #[test]
    fn detects_supported_audio_and_media_extensions_case_insensitively() {
        assert!(is_audio_file(Path::new("track.FLAC")));
        assert!(is_audio_file(Path::new("voice.opus")));
        assert!(is_audio_file(Path::new("album.APE")));
        assert!(is_audio_file(Path::new("master.WV")));
        assert!(is_audio_file(Path::new("archive.aiff")));
        assert!(is_audio_file(Path::new("lossless.mka")));
        assert!(!is_audio_file(Path::new("movie.mp4")));
        assert!(is_media_file(Path::new("movie.mp4")));
        assert!(is_media_file(Path::new("track.mp3")));
        assert!(!is_media_file(Path::new("cover.jpg")));
    }

    #[test]
    fn sorts_numbered_files_naturally() {
        let mut files = vec!["video10.mp4", "video2.mp4", "video1.mp4"];
        files.sort_by(|left, right| natural_cmp(left, right));

        assert_eq!(files, ["video1.mp4", "video2.mp4", "video10.mp4"]);
    }

    #[test]
    fn natural_sort_is_case_insensitive() {
        assert_eq!(natural_cmp("Episode2.MKV", "episode10.mkv"), Ordering::Less);
    }

    #[test]
    fn natural_sort_obeys_total_order_properties_on_mixed_names() {
        let fragments = [
            "",
            "a",
            "A",
            "z",
            "Ж",
            "ёж",
            "é",
            "0",
            "00",
            "1",
            "01",
            "9",
            "10",
            "999999999999999999999999999999",
            "-",
            " ",
            ".",
            "_",
        ];
        let mut names = Vec::new();
        for left in fragments {
            for right in fragments {
                names.push(format!("{left}{right}.mkv"));
            }
        }

        for left in &names {
            assert_eq!(natural_cmp(left, left), Ordering::Equal);
            for right in &names {
                assert_eq!(
                    natural_cmp(left, right),
                    natural_cmp(right, left).reverse(),
                    "left={left:?}, right={right:?}"
                );
            }
        }

        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..20_000 {
            let mut next_index = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as usize % names.len()
            };
            let left = &names[next_index()];
            let middle = &names[next_index()];
            let right = &names[next_index()];
            if natural_cmp(left, middle) != Ordering::Greater
                && natural_cmp(middle, right) != Ordering::Greater
            {
                assert_ne!(
                    natural_cmp(left, right),
                    Ordering::Greater,
                    "left={left:?}, middle={middle:?}, right={right:?}"
                );
            }
        }

        names.sort_by(|left, right| natural_cmp(left, right));
        assert!(
            names
                .windows(2)
                .all(|pair| natural_cmp(&pair[0], &pair[1]) != Ordering::Greater)
        );
        let once = names.clone();
        names.sort_by(|left, right| natural_cmp(left, right));
        assert_eq!(names, once);
    }

    #[test]
    fn opens_directory_at_first_naturally_sorted_media_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("oxide-playlist-{}-{unique}", std::process::id()));

        fs::create_dir(&directory).expect("temporary playlist directory must be created");
        for name in [
            "video10.mp4",
            "track3.flac",
            "video2.mkv",
            "notes.txt",
            "video1.webm",
        ] {
            fs::write(directory.join(name), []).expect("temporary file must be created");
        }

        let single = DirectoryPlaylist::from_file(&directory.join("video10.mp4"))
            .expect("an existing supported file must open immediately");
        assert_eq!(single.items(), ["video10.mp4"]);

        let playlist =
            DirectoryPlaylist::load_from_directory_cancellable(&directory, &AtomicBool::new(false))
                .expect("directory with videos must produce a playlist")
                .expect("ordinary test scan must not be cancelled");

        assert_eq!(
            playlist.items(),
            ["track3.flac", "video1.webm", "video2.mkv", "video10.mp4"]
        );
        assert_eq!(
            playlist
                .current_path()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("track3.flac")
        );

        fs::remove_dir_all(directory).expect("temporary playlist directory must be removed");
    }

    #[test]
    fn cancelled_scan_returns_without_touching_the_directory() {
        let cancelled = AtomicBool::new(true);
        let result = DirectoryPlaylist::load_from_directory_cancellable(
            Path::new("directory-that-must-not-be-read"),
            &cancelled,
        )
        .expect("cancellation is not a filesystem error");
        assert!(result.is_none());
    }

    #[test]
    fn adjacent_navigation_only_wraps_when_requested() {
        let mut playlist = DirectoryPlaylist {
            files: ["one.mp4", "two.mp4", "three.mp4"]
                .map(PathBuf::from)
                .to_vec(),
            current_index: 2,
            omitted_count: 0,
        };

        assert_eq!(playlist.move_next(false), None);
        assert_eq!(playlist.move_next(true), Some(PathBuf::from("one.mp4")));
        assert_eq!(
            playlist.move_previous(true),
            Some(PathBuf::from("three.mp4"))
        );
    }

    #[test]
    fn shuffle_and_restore_keep_current_video_selected() {
        let mut playlist = DirectoryPlaylist {
            files: ["video1.mp4", "video2.mp4", "video3.mp4", "video4.mp4"]
                .map(PathBuf::from)
                .to_vec(),
            current_index: 1,
            omitted_count: 0,
        };
        let current = playlist.current_path().cloned();

        playlist.set_shuffled(true);
        assert_eq!(playlist.current_path(), current.as_ref());
        assert_eq!(playlist.current_index(), 0);

        playlist.set_shuffled(false);
        assert_eq!(
            playlist.items(),
            ["video1.mp4", "video2.mp4", "video3.mp4", "video4.mp4"]
        );
        assert_eq!(playlist.current_path(), current.as_ref());
    }

    #[test]
    fn removing_unavailable_video_selects_the_following_item() {
        let mut playlist = DirectoryPlaylist {
            files: ["one.mp4", "missing.mp4", "three.mp4"]
                .map(PathBuf::from)
                .to_vec(),
            current_index: 1,
            omitted_count: 0,
        };

        assert_eq!(
            playlist.remove_current_and_select(true, false),
            Some(PathBuf::from("three.mp4"))
        );
        assert_eq!(playlist.items(), ["one.mp4", "three.mp4"]);

        assert_eq!(playlist.remove_current_and_select(true, false), None);
    }

    #[test]
    fn selects_playlist_item_by_index_without_changing_on_invalid_index() {
        let mut playlist = DirectoryPlaylist {
            files: vec![PathBuf::from("one.mp4"), PathBuf::from("two.mp4")],
            current_index: 0,
            omitted_count: 0,
        };

        assert_eq!(playlist.select(1), Some(PathBuf::from("two.mp4")));
        assert_eq!(playlist.current_index(), 1);
        assert_eq!(playlist.select(2), None);
        assert_eq!(playlist.current_index(), 1);
    }

    #[test]
    fn removing_playlist_items_keeps_a_valid_current_selection() {
        let mut playlist = DirectoryPlaylist {
            files: ["one.mp4", "two.mp4", "three.mp4"]
                .map(PathBuf::from)
                .to_vec(),
            current_index: 1,
            omitted_count: 0,
        };

        assert_eq!(playlist.remove(0), Some(false));
        assert_eq!(playlist.current_path(), Some(&PathBuf::from("two.mp4")));
        assert_eq!(playlist.current_index(), 0);

        assert_eq!(playlist.remove(0), Some(true));
        assert_eq!(playlist.current_path(), Some(&PathBuf::from("three.mp4")));
        assert_eq!(playlist.current_index(), 0);

        assert_eq!(playlist.remove(0), Some(true));
        assert_eq!(playlist.current_path(), None);
        assert_eq!(playlist.remove(0), None);
    }

    #[test]
    fn bounded_playlist_keeps_memory_and_entry_counts_fixed() {
        let limits = PlaylistLimits {
            files: 3,
            path_bytes: 64,
            label_bytes: 32,
            single_path_bytes: 32,
        };
        let mut files = PlaylistAccumulator::new(limits);
        assert!(files.push(PathBuf::from("selected.mp4")));
        assert!(files.push(PathBuf::from("two.mp4")));
        assert!(!files.push(PathBuf::from("this-name-is-too-long-for-budget.mp4")));
        assert!(files.push(PathBuf::from("three.mp4")));
        assert!(!files.push(PathBuf::from("four.mp4")));

        let files = files.finish();
        assert_eq!(files.files.len(), 3);
        assert_eq!(files.omitted_count, 2);
        assert!(files.files.contains(&PathBuf::from("selected.mp4")));
    }

    #[test]
    fn playlist_limit_notice_explains_the_result_in_plain_language() {
        let playlist = DirectoryPlaylist {
            files: ["one.mp4", "two.mp4"].map(PathBuf::from).to_vec(),
            current_index: 0,
            omitted_count: 3,
        };

        assert_eq!(
            playlist.limit_notice().as_deref(),
            Some("Файлов в очереди: 2. Ещё 3 не добавлено, чтобы плеер работал быстро.")
        );
    }
}
