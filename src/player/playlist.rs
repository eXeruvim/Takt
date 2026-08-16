use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv", "wmv", "mpg", "mpeg", "ogv",
];

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            VIDEO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
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
        if !is_video_file(path) {
            bail!("Неподдерживаемый формат видео: {}", path.display());
        }

        let directory = path.parent().context("Не удалось определить папку видео")?;
        let mut files = fs::read_dir(directory)
            .with_context(|| format!("Не удалось прочитать папку: {}", directory.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|candidate| candidate.is_file() && is_video_file(candidate))
            .collect::<Vec<_>>();

        files.sort_by(|left, right| {
            let left = left.file_name().unwrap_or_default().to_string_lossy();
            let right = right.file_name().unwrap_or_default().to_string_lossy();
            natural_cmp(&left, &right)
        });

        let current_index = files
            .iter()
            .position(|candidate| candidate == path)
            .context("Открытый файл отсутствует в сформированном плейлисте")?;

        Ok(Self {
            files,
            current_index,
        })
    }

    pub fn next_path(&self) -> Option<&PathBuf> {
        self.files.get(self.current_index + 1)
    }

    pub fn previous_path(&self) -> Option<&PathBuf> {
        self.current_index
            .checked_sub(1)
            .and_then(|index| self.files.get(index))
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
    fn sorts_numbered_files_naturally() {
        let mut files = vec!["video10.mp4", "video2.mp4", "video1.mp4"];
        files.sort_by(|left, right| natural_cmp(left, right));

        assert_eq!(files, ["video1.mp4", "video2.mp4", "video10.mp4"]);
    }

    #[test]
    fn natural_sort_is_case_insensitive() {
        assert_eq!(natural_cmp("Episode2.MKV", "episode10.mkv"), Ordering::Less);
    }
}
