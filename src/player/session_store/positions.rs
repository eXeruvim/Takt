use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
};

pub(super) const MAX_POSITIONS: usize = 1000;
// Paths are held once by the lookup index and once by the recency queue.
pub(super) const MAX_POSITION_PATH_BYTES: usize = 1024 * 1024;
pub(super) const MAX_PATH_BYTES: usize = 128 * 1024;

pub(super) fn path_bytes(path: &Path) -> Option<usize> {
    let text = path.to_str()?;
    (!text.is_empty() && text.len() <= MAX_PATH_BYTES).then_some(text.len())
}

#[derive(Debug, Default)]
pub(super) struct Positions {
    entries: HashMap<PathBuf, f64>,
    // Oldest first. A second path copy buys indexed lookup while keeping stable
    // serialization order. `path_bytes` accounts for both allocations.
    order: VecDeque<PathBuf>,
    path_bytes: usize,
}

#[derive(Default)]
pub(super) struct Update {
    pub changed: bool,
    pub discarded: bool,
}

impl Positions {
    pub fn get(&self, path: &Path) -> Option<f64> {
        self.entries.get(path).copied()
    }

    pub fn remove(&mut self, path: &Path) -> bool {
        if self.entries.remove(path).is_none() {
            return false;
        }
        let index = self
            .order
            .iter()
            .position(|entry| entry == path)
            .expect("indexed path must have recency entry");
        let removed = self.order.remove(index).expect("existing index");
        self.path_bytes -= path_bytes(&removed).expect("validated path") * 2;
        true
    }

    pub fn touch(&mut self, path: &Path) {
        if self.order.back().is_some_and(|entry| entry == path) || !self.entries.contains_key(path)
        {
            return;
        }
        let index = self
            .order
            .iter()
            .position(|entry| entry == path)
            .expect("indexed path must have recency entry");
        let entry = self.order.remove(index).expect("existing index");
        self.order.push_back(entry);
    }

    pub fn record(&mut self, path: &Path, seconds: f64, protected: &[PathBuf]) -> Update {
        let Some(bytes) = path_bytes(path) else {
            return Update {
                discarded: true,
                ..Update::default()
            };
        };
        if let Some(previous) = self.entries.get_mut(path) {
            let changed = *previous != seconds;
            *previous = seconds;
            self.touch(path);
            return Update {
                changed,
                discarded: false,
            };
        }

        let mut discarded = false;
        while self.entries.len() >= MAX_POSITIONS
            || self.path_bytes + bytes * 2 > MAX_POSITION_PATH_BYTES
        {
            // Legacy V1 has no reliable order. Preserve its bounded recent list
            // while parsing; normal runtime/V2 parsing pass no protected paths.
            let Some(index) = self
                .order
                .iter()
                .position(|entry| !protected.contains(entry))
            else {
                return Update {
                    changed: discarded,
                    discarded: true,
                };
            };
            let entry = self.order.remove(index).expect("existing index");
            self.entries
                .remove(&entry)
                .expect("recency entry must be indexed");
            self.path_bytes -= path_bytes(&entry).expect("validated path") * 2;
            discarded = true;
        }

        let path = path.to_owned();
        self.order.push_back(path.clone());
        self.entries.insert(path, seconds);
        self.path_bytes += bytes * 2;
        Update {
            changed: true,
            discarded,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Path, f64)> {
        self.order.iter().map(|path| {
            (
                path.as_path(),
                *self
                    .entries
                    .get(path)
                    .expect("recency entry must be indexed"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_history_evicts_oldest_but_refreshing_a_file_keeps_it() {
        let mut positions = Positions::default();
        for index in 0..MAX_POSITIONS {
            positions.record(Path::new(&format!("{index}.mp4")), 30.0, &[]);
        }
        positions.touch(Path::new("0.mp4"));
        assert!(positions.record(Path::new("new.mp4"), 40.0, &[]).discarded);
        assert_eq!(positions.entries.len(), MAX_POSITIONS);
        assert_eq!(positions.get(Path::new("0.mp4")), Some(30.0));
        assert!(positions.get(Path::new("1.mp4")).is_none());
        assert_eq!(positions.get(Path::new("new.mp4")), Some(40.0));
    }

    #[test]
    fn updating_current_position_does_not_grow_storage() {
        let mut positions = Positions::default();
        let path = Path::new("track.flac");
        positions.record(path, 30.0, &[]);
        for seconds in 0..100_000 {
            positions.record(path, f64::from(seconds), &[]);
        }
        assert_eq!(positions.entries.len(), 1);
        assert_eq!(positions.order.len(), 1);
        assert_eq!(positions.path_bytes, "track.flac".len() * 2);
        assert!(!positions.record(path, 99_999.0, &[]).changed);
    }

    #[test]
    fn path_byte_budget_applies_even_with_few_entries() {
        let mut positions = Positions::default();
        for index in 0..20 {
            let path = format!("{index:02}{}", "x".repeat(MAX_PATH_BYTES - 2));
            positions.record(Path::new(&path), 30.0, &[]);
            assert!(positions.path_bytes <= MAX_POSITION_PATH_BYTES);
        }
        assert_eq!(
            positions.entries.len(),
            MAX_POSITION_PATH_BYTES / (MAX_PATH_BYTES * 2)
        );
        let oversized = "x".repeat(MAX_PATH_BYTES + 1);
        assert!(positions.record(Path::new(&oversized), 30.0, &[]).discarded);
        assert!(positions.get(Path::new(&oversized)).is_none());
    }

    #[test]
    fn removal_and_reordering_keep_byte_accounting_exact() {
        let mut positions = Positions::default();
        positions.record(Path::new("one"), 30.0, &[]);
        positions.record(Path::new("two"), 40.0, &[]);
        positions.record(Path::new("one"), 50.0, &[]);
        assert_eq!(positions.path_bytes, 12);
        assert_eq!(
            positions.iter().map(|(path, _)| path).collect::<Vec<_>>(),
            [Path::new("two"), Path::new("one")]
        );
        assert!(positions.remove(Path::new("two")));
        assert!(!positions.remove(Path::new("missing")));
        assert_eq!(positions.path_bytes, 6);
    }
}
