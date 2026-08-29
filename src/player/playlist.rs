use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

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
}

impl DirectoryPlaylist {
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.is_file() {
            bail!("Файл не найден: {}", path.display());
        }
        if !is_media_file(path) {
            bail!("Неподдерживаемый формат медиа: {}", path.display());
        }

        let directory = path
            .parent()
            .context("Не удалось определить папку медиафайла")?;
        let files = read_media_files(directory)?;

        let current_index = files
            .iter()
            .position(|candidate| candidate == path)
            .context("Открытый файл отсутствует в сформированном плейлисте")?;

        Ok(Self {
            files,
            current_index,
        })
    }

    pub fn load_from_directory(directory: &Path) -> Result<Self> {
        if !directory.is_dir() {
            bail!("Папка не найдена: {}", directory.display());
        }

        let files = read_media_files(directory)?;
        if files.is_empty() {
            bail!(
                "В папке нет поддерживаемых видео или аудио: {}",
                directory.display()
            );
        }

        Ok(Self {
            files,
            current_index: 0,
        })
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
        self.files
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }
}

fn read_media_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(directory)
        .with_context(|| format!("Не удалось прочитать папку: {}", directory.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|candidate| candidate.is_file() && is_media_file(candidate))
        .collect::<Vec<_>>();

    sort_paths_naturally(&mut files);

    Ok(files)
}

fn sort_paths_naturally(files: &mut [PathBuf]) {
    files.sort_by(|left, right| {
        let left = left.file_name().unwrap_or_default().to_string_lossy();
        let right = right.file_name().unwrap_or_default().to_string_lossy();
        natural_cmp(&left, &right)
    });
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
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek(), right_chars.peek()) {
            (Some(left_char), Some(right_char))
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() =>
            {
                let left_number = take_digits(&mut left_chars);
                let right_number = take_digits(&mut right_chars);
                let ordering = compare_digit_runs(&left_number, &right_number);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(_), Some(_)) => {
                let ordering = left_chars.next().cmp(&right_chars.next());
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        if let Some(character) = chars.next() {
            digits.push(character);
        }
    }
    digits
}

fn compare_digit_runs(left: &str, right: &str) -> Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    let left_significant = if left_trimmed.is_empty() {
        "0"
    } else {
        left_trimmed
    };
    let right_significant = if right_trimmed.is_empty() {
        "0"
    } else {
        right_trimmed
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

        let playlist = DirectoryPlaylist::load_from_directory(&directory)
            .expect("directory with videos must produce a playlist");

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
    fn adjacent_navigation_only_wraps_when_requested() {
        let mut playlist = DirectoryPlaylist {
            files: ["one.mp4", "two.mp4", "three.mp4"]
                .map(PathBuf::from)
                .to_vec(),
            current_index: 2,
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
        };

        assert_eq!(playlist.select(1), Some(PathBuf::from("two.mp4")));
        assert_eq!(playlist.current_index(), 1);
        assert_eq!(playlist.select(2), None);
        assert_eq!(playlist.current_index(), 1);
    }
}
