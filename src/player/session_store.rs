use crate::storage::Persistence;
use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};
use std::{
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

mod positions;
use positions::{MAX_PATH_BYTES, Positions, path_bytes};

const STATE_HEADER: &str = "OXIDE_PLAYBACK_STATE_V2";
const LEGACY_HEADER: &str = "OXIDE_PLAYBACK_STATE_V1";
const MAX_RECENT_FILES: usize = 10;
const MAX_RECENT_PATH_BYTES: usize = 256 * 1024;
const RESUME_MIN_SECONDS: f64 = 5.0;
const RESUME_END_GUARD_SECONDS: f64 = 10.0;

struct SaveRequest {
    content: String,
    copies: usize,
}

#[derive(Debug)]
struct BackgroundSave {
    requests: Option<Sender<SaveRequest>>,
    results: Receiver<Result<(), String>>,
    worker: Option<JoinHandle<()>>,
}

impl BackgroundSave {
    fn spawn(persistence: Arc<Mutex<Persistence>>) -> io::Result<Self> {
        // One in-flight request is enough: SessionStore keeps later changes
        // dirty and submits their newest snapshot after this result arrives.
        let (requests, pending) = bounded::<SaveRequest>(1);
        let (completed, results) = bounded(1);
        let worker = thread::Builder::new()
            .name("oxide-session-save".into())
            .spawn(move || {
                while let Ok(request) = pending.recv() {
                    let result = persistence
                        .lock()
                        .map_err(|_| "Хранилище истории повреждено после panic".to_owned())
                        .and_then(|mut persistence| {
                            for _ in 0..request.copies {
                                persistence
                                    .save(&request.content)
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        });
                    if completed.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            results,
            worker: Some(worker),
        })
    }

    fn submit(&self, request: SaveRequest) -> Result<(), String> {
        self.requests
            .as_ref()
            .ok_or_else(|| "Фоновое сохранение истории остановлено".to_owned())?
            .try_send(request)
            .map_err(|error| match error {
                crossbeam_channel::TrySendError::Full(_) => {
                    "Предыдущее сохранение истории ещё выполняется".to_owned()
                }
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    "Фоновое сохранение истории недоступно".to_owned()
                }
            })
    }
}

impl Drop for BackgroundSave {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            eprintln!("Session save worker panicked during shutdown");
        }
    }
}

#[derive(Debug)]
pub struct SessionStore {
    recent: Vec<PathBuf>,
    positions: Positions,
    persistence: Arc<Mutex<Persistence>>,
    background_save: Option<BackgroundSave>,
    background_pending: bool,
    last_save_error: Option<String>,
    dirty: bool,
    forget_previous_pending: bool,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            positions: Positions::default(),
            persistence: Arc::new(Mutex::new(Persistence::default())),
            background_save: None,
            background_pending: false,
            last_save_error: None,
            dirty: false,
            forget_previous_pending: false,
        }
    }
}

impl SessionStore {
    pub fn load_default() -> Self {
        let (persistence, mut store) = Persistence::load(default_storage_path(), Self::parse);
        store.dirty |= persistence.needs_repair();
        store.attach_persistence(persistence);
        store
    }

    #[cfg(test)]
    fn load_from(storage_path: &Path) -> std::io::Result<Self> {
        let (persistence, mut store) = Persistence::open(storage_path, Self::parse)?;
        store.dirty |= persistence.needs_repair();
        store.attach_persistence(persistence);
        Ok(store)
    }

    fn attach_persistence(&mut self, persistence: Persistence) {
        self.persistence = Arc::new(Mutex::new(persistence));
        self.background_save = BackgroundSave::spawn(Arc::clone(&self.persistence)).ok();
    }

    fn parse(content: &str) -> std::io::Result<Self> {
        let mut lines = content.lines();
        let header = lines.next().unwrap_or_default();
        let legacy = header == LEGACY_HEADER;
        if header != STATE_HEADER && !legacy {
            return Err(std::io::Error::new(
                if header.starts_with("OXIDE_PLAYBACK_STATE_V") {
                    std::io::ErrorKind::Unsupported
                } else {
                    std::io::ErrorKind::InvalidData
                },
                "Неизвестный или повреждённый формат истории воспроизведения",
            ));
        }

        let mut store = Self {
            dirty: legacy,
            ..Self::default()
        };
        // Read the small recent list first, even if legacy records are reordered.
        // Never accumulate an unbounded vector and truncate it afterwards.
        for line in lines.clone() {
            if let Some(encoded) = line.strip_prefix("recent\t") {
                if let Some(path) = decode_path(encoded) {
                    if store.recent.contains(&path) {
                        store.dirty = true;
                        continue;
                    }
                    let bytes = path_bytes(&path).expect("decoded path");
                    if store.recent.len() < MAX_RECENT_FILES
                        && store
                            .recent
                            .iter()
                            .filter_map(|path| path_bytes(path))
                            .sum::<usize>()
                            + bytes
                            <= MAX_RECENT_PATH_BYTES
                    {
                        store.recent.push(path);
                    } else {
                        store.dirty = true;
                    }
                } else {
                    store.dirty = true;
                }
            }
        }
        for line in lines {
            let mut fields = line.split('\t');
            if fields.next() == Some("position") {
                let seconds = fields.next().and_then(|value| value.parse::<f64>().ok());
                let path = fields.next().and_then(decode_path);
                if fields.next().is_none()
                    && let (Some(seconds), Some(path)) = (seconds, path)
                    && seconds.is_finite()
                    && seconds >= RESUME_MIN_SECONDS
                {
                    let protected = if legacy { store.recent.as_slice() } else { &[] };
                    store.dirty |= store.positions.record(&path, seconds, protected).discarded;
                } else {
                    store.dirty = true;
                }
            }
        }
        if legacy {
            for path in store.recent.iter().rev() {
                store.positions.touch(path);
            }
        }
        Ok(store)
    }

    pub fn recent(&self) -> &[PathBuf] {
        &self.recent
    }

    pub fn take_storage_notice(&mut self) -> Option<String> {
        self.persistence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take_notice()
    }

    /// Surface an error once, not on every periodic save. Keep dirty for retry.
    pub fn save_and_take_error(&mut self) -> Option<String> {
        match self.save_if_dirty() {
            Ok(()) => {
                self.last_save_error = None;
                None
            }
            Err(error) => {
                let message = format!("Не удалось сохранить историю и позиции: {error}");
                if self.last_save_error.as_ref() == Some(&message) {
                    return None;
                }
                self.last_save_error = Some(message.clone());
                Some(message)
            }
        }
    }

    /// Queue the newest snapshot without blocking playback on disk flushes.
    /// There is at most one in-flight write; changes made while it runs remain
    /// dirty and are coalesced into the next snapshot.
    pub fn save_in_background_and_take_error(&mut self) -> Option<String> {
        let completed = self.poll_background_result();
        let notice = completed.and_then(|result| self.apply_save_result(result));

        if self.dirty && !self.background_pending {
            let request = self.save_request();
            match self
                .background_save
                .as_ref()
                .map(|background| background.submit(request))
            {
                Some(Ok(())) => {
                    self.background_pending = true;
                    self.dirty = false;
                    self.forget_previous_pending = false;
                }
                Some(Err(error)) => return self.apply_save_result(Err(error)),
                None => {
                    return self.apply_save_result(Err(
                        "Не удалось запустить фоновое сохранение истории".to_owned(),
                    ));
                }
            }
        }

        notice
    }

    pub fn record_open(&mut self, path: &Path) {
        if path_bytes(path).is_none() {
            return;
        }
        self.positions.touch(path);
        let path = path.to_owned();
        self.recent.retain(|existing| existing != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT_FILES);
        while self
            .recent
            .iter()
            .filter_map(|path| path_bytes(path))
            .sum::<usize>()
            > MAX_RECENT_PATH_BYTES
        {
            self.recent.pop();
        }
        self.dirty = true;
    }

    pub fn record_position(&mut self, path: &Path, seconds: f64, duration: f64) {
        if !seconds.is_finite() || seconds < RESUME_MIN_SECONDS {
            if self.positions.remove(path) {
                self.dirty = true;
            }
            return;
        }
        if duration.is_finite() && duration > 0.0 && duration - seconds <= RESUME_END_GUARD_SECONDS
        {
            self.clear_position(path);
            return;
        }

        self.dirty |= self.positions.record(path, seconds, &[]).changed;
    }

    pub fn clear_position(&mut self, path: &Path) {
        if self.positions.remove(path) {
            self.dirty = true;
        }
    }

    /// Forget every recent file and resume position without touching media.
    ///
    /// Two commits are intentional: the first publishes the empty state and
    /// the second replaces the recovery copy that still contains the previous
    /// state. A successful return therefore cannot resurrect cleared history
    /// by falling back to the `.bak` file after later corruption.
    pub fn clear_playback_data(&mut self) -> std::io::Result<()> {
        self.recent.clear();
        self.positions.clear();
        self.dirty = true;
        self.forget_previous_pending = true;
        self.save_if_dirty()
    }

    pub fn resume_position(&self, path: &Path, duration: f64) -> Option<f64> {
        let position = self.positions.get(path)?;
        (position >= RESUME_MIN_SECONDS
            && (!duration.is_finite()
                || duration <= 0.0
                || duration - position > RESUME_END_GUARD_SECONDS))
            .then_some(position)
    }

    pub fn save_if_dirty(&mut self) -> std::io::Result<()> {
        self.finish_background_save()?;
        if !self.dirty {
            return Ok(());
        }
        let request = self.save_request();
        let mut persistence = self
            .persistence
            .lock()
            .map_err(|_| io::Error::other("Хранилище истории повреждено после panic"))?;
        for _ in 0..request.copies {
            persistence.save(&request.content)?;
        }
        self.dirty = false;
        self.forget_previous_pending = false;
        Ok(())
    }

    fn save_request(&self) -> SaveRequest {
        let mut content = String::from(STATE_HEADER);
        content.push('\n');
        for path in &self.recent {
            let _ = writeln!(content, "recent\t{}", encode_path(path));
        }
        // V2 persists oldest-to-newest order, without wall-clock timestamps.
        for (path, seconds) in self.positions.iter() {
            let _ = writeln!(content, "position\t{seconds:.3}\t{}", encode_path(path));
        }
        SaveRequest {
            content,
            // The first empty commit leaves the former primary as recovery
            // data. A second commit makes recovery forget it as well.
            copies: if self.forget_previous_pending { 2 } else { 1 },
        }
    }

    fn poll_background_result(&mut self) -> Option<Result<(), String>> {
        if !self.background_pending {
            return None;
        }
        let result = match self.background_save.as_ref()?.results.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                Err("Фоновое сохранение истории недоступно".to_owned())
            }
        };
        self.background_pending = false;
        Some(result)
    }

    fn finish_background_save(&mut self) -> io::Result<()> {
        if !self.background_pending {
            return Ok(());
        }
        let result = self
            .background_save
            .as_ref()
            .ok_or_else(|| io::Error::other("Фоновое сохранение истории недоступно"))?
            .results
            .recv()
            .map_err(|_| io::Error::other("Фоновое сохранение истории недоступно"))?;
        self.background_pending = false;
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                self.dirty = true;
                Err(io::Error::other(error))
            }
        }
    }

    fn apply_save_result(&mut self, result: Result<(), String>) -> Option<String> {
        match result {
            Ok(()) => {
                self.last_save_error = None;
                None
            }
            Err(error) => {
                self.dirty = true;
                let message = format!("Не удалось сохранить историю и позиции: {error}");
                if self.last_save_error.as_ref() == Some(&message) {
                    return None;
                }
                self.last_save_error = Some(message.clone());
                Some(message)
            }
        }
    }
}

fn default_storage_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .map(|directory| directory.join("Oxide").join("playback-state.txt"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("state"))
            })
            .map(|directory| directory.join("oxide").join("playback-state.txt"))
    }
}

fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .as_bytes()
        .iter()
        .fold(String::new(), |mut encoded, byte| {
            let _ = write!(encoded, "{byte:02x}");
            encoded
        })
}

fn decode_path(value: &str) -> Option<PathBuf> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES * 2 || !value.len().is_multiple_of(2) {
        return None;
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let path = String::from_utf8(bytes).ok().map(PathBuf::from)?;
    path_bytes(&path)?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::{SessionStore, decode_path, encode_path};
    use crate::storage::tests::TestDirectory;
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[test]
    fn legacy_history_migrates_without_losing_positions_and_keeps_backup() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let track = Path::new("старый трек.mp3");
        let old = format!(
            "OXIDE_PLAYBACK_STATE_V1\nrecent\t{}\nposition\t42.500\t{}\n",
            encode_path(track),
            encode_path(track)
        );
        std::fs::write(&path, &old).unwrap();
        let mut store = SessionStore::load_from(&path).unwrap();
        assert!(store.dirty);
        assert_eq!(store.resume_position(track, 100.0), Some(42.5));
        store.save_if_dirty().unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains(super::STATE_HEADER)
        );
        assert!(
            std::fs::read_to_string(files.path("state.txt.bak"))
                .unwrap()
                .contains(&old)
        );
        let loaded = SessionStore::load_from(&path).unwrap();
        assert_eq!(loaded.resume_position(track, 100.0), Some(42.5));
        assert!(!loaded.dirty);
    }

    #[test]
    fn position_recency_survives_restart_and_controls_eviction() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let mut store = SessionStore::load_from(&path).unwrap();
        for index in 0..super::positions::MAX_POSITIONS {
            store.record_position(Path::new(&format!("{index}.mp4")), 30.0, 100.0);
        }
        store.record_open(Path::new("0.mp4"));
        store.save_if_dirty().unwrap();
        let mut loaded = SessionStore::load_from(&path).unwrap();
        loaded.record_position(Path::new("new.mp4"), 40.0, 100.0);
        assert_eq!(
            loaded.resume_position(Path::new("0.mp4"), 100.0),
            Some(30.0)
        );
        assert_eq!(loaded.resume_position(Path::new("1.mp4"), 100.0), None);
        assert_eq!(
            loaded.positions.iter().count(),
            super::positions::MAX_POSITIONS
        );
    }

    #[test]
    fn oversized_legacy_history_keeps_recent_positions_even_when_recent_lines_come_last() {
        let started = std::time::Instant::now();
        let mut content = String::from("OXIDE_PLAYBACK_STATE_V1\n");
        let keep = Path::new("old-but-recent.mp4");
        writeln!(content, "position\t30\t{}", encode_path(keep)).unwrap();
        for index in 0..20_000 {
            writeln!(
                content,
                "position\t40\t{}",
                encode_path(Path::new(&format!("{index}.mp4")))
            )
            .unwrap();
        }
        writeln!(content, "recent\t{}", encode_path(keep)).unwrap();
        let store = SessionStore::parse(&content).unwrap();
        assert!(store.dirty);
        assert_eq!(store.resume_position(keep, 100.0), Some(30.0));
        assert_eq!(
            store.resume_position(Path::new("19999.mp4"), 100.0),
            Some(40.0)
        );
        assert_eq!(
            store.positions.iter().count(),
            super::positions::MAX_POSITIONS
        );
        assert_eq!(store.positions.iter().last().unwrap().0, keep);
        eprintln!(
            "legacy retention: 20001 positions reduced to 1000, build+parse {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn encoded_path_limits_reject_large_or_invalid_paths_before_decoding() {
        assert!(decode_path("").is_none());
        assert!(decode_path("a").is_none());
        assert!(decode_path("gg").is_none());
        assert!(decode_path("ff").is_none());
        let oversized = "61".repeat(super::MAX_PATH_BYTES + 1);
        assert!(decode_path(&oversized).is_none());
        let maximum = "x".repeat(super::MAX_PATH_BYTES);
        assert_eq!(
            decode_path(&encode_path(Path::new(&maximum))).unwrap(),
            Path::new(&maximum)
        );
    }

    #[test]
    fn malformed_history_records_preserve_bounded_finite_state() {
        let paths = [
            encode_path(Path::new("track.mp3")),
            encode_path(Path::new("Видео/ролик 10.mkv")),
            String::new(),
            "0".into(),
            "gg".into(),
            "ff".into(),
            "61".repeat(512),
        ];
        let seconds = [
            "NaN",
            "inf",
            "-inf",
            "-1",
            "0",
            "4.999",
            "5",
            "42.5",
            "1e308",
            "not-a-number",
        ];
        let mut state = 0xa076_1d64_78bd_642f_u64;

        for _ in 0..1_000 {
            let mut content = format!("{}\n", super::STATE_HEADER);
            for _ in 0..32 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let path = &paths[state as usize % paths.len()];
                state = state.rotate_left(23).wrapping_add(0xe703_7ed1_a0b4_28db);
                let time = seconds[state as usize % seconds.len()];
                match state % 5 {
                    0 => content.push_str(&format!("recent\t{path}\n")),
                    1 => content.push_str(&format!("position\t{time}\t{path}\n")),
                    2 => content.push_str(&format!("position\t{time}\t{path}\textra\n")),
                    3 => content.push_str("position\n"),
                    _ => content.push_str("unknown\tignored\n"),
                }
            }

            let store = SessionStore::parse(&content).expect("valid header must remain readable");
            assert!(store.recent.len() <= super::MAX_RECENT_FILES);
            for (index, path) in store.recent.iter().enumerate() {
                assert!(super::path_bytes(path).is_some());
                assert!(!store.recent[index + 1..].contains(path));
            }
            assert!(store.positions.iter().count() <= super::positions::MAX_POSITIONS);
            for (path, position) in store.positions.iter() {
                assert!(super::path_bytes(path).is_some());
                assert!(position.is_finite());
                assert!(position >= super::RESUME_MIN_SECONDS);
            }
        }
    }

    #[test]
    fn recent_records_are_deduplicated_and_bounded_while_parsing() {
        let mut content = format!("{}\n", super::STATE_HEADER);
        for index in 0..20_000 {
            let path = encode_path(Path::new(&format!("{}.mp4", index / 2)));
            writeln!(content, "recent\t{path}").unwrap();
        }
        let store = SessionStore::parse(&content).unwrap();
        assert!(store.dirty);
        assert_eq!(store.recent.len(), super::MAX_RECENT_FILES);
        assert_eq!(store.recent[0], Path::new("0.mp4"));
        assert_eq!(store.recent[9], Path::new("9.mp4"));
    }

    #[test]
    fn long_path_budgets_keep_serialized_state_below_storage_limit() {
        let files = TestDirectory::new();
        let path = files.path("state.txt");
        let mut store = SessionStore::load_from(&path).unwrap();
        // Synthetic paths only: no attempt to open or create these media files.
        for index in 0..20 {
            let text = format!("{index:02}{}", "x".repeat(super::MAX_PATH_BYTES - 2));
            store.record_open(Path::new(&text));
            store.record_position(Path::new(&text), 30.0, 100.0);
        }
        assert!(
            store
                .recent
                .iter()
                .filter_map(|path| super::path_bytes(path))
                .sum::<usize>()
                <= super::MAX_RECENT_PATH_BYTES
        );
        store.save_if_dirty().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 3 * 1024 * 1024);
        let loaded = SessionStore::load_from(&path).unwrap();
        assert_eq!(
            loaded.positions.iter().count(),
            store.positions.iter().count()
        );
        assert_eq!(loaded.recent(), store.recent());
        assert!(!loaded.dirty);
    }

    #[test]
    fn path_encoding_round_trips_unicode_without_delimiter_escaping() {
        let path = Path::new(r"C:\Музыка\трек с пробелом.flac");
        assert_eq!(decode_path(&encode_path(path)), Some(path.to_owned()));
    }

    #[test]
    fn history_and_resume_position_round_trip_and_recover_together() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let track = Path::new("Музыка/мой трек.flac");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(track);
        store.record_position(track, 42.5, 100.0);
        store.save_if_dirty().unwrap();
        store.record_position(track, 60.0, 100.0);
        store.save_if_dirty().unwrap();
        assert_eq!(
            SessionStore::load_from(&path)
                .unwrap()
                .resume_position(track, 100.0),
            Some(60.0)
        );
        std::fs::write(&path, b"broken").unwrap();
        let mut recovered = SessionStore::load_from(&path).unwrap();
        assert_eq!(recovered.recent(), &[track.to_owned()]);
        assert_eq!(recovered.resume_position(track, 100.0), Some(42.5));
        assert!(recovered.take_storage_notice().is_some());
        recovered.save_if_dirty().unwrap();
        assert!(!recovered.dirty);
    }

    #[test]
    fn periodic_save_never_waits_for_the_storage_device() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let first = Path::new("первый трек.mp3");
        let second = Path::new("второй трек.mp3");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(first);

        // Hold the exact mutex the writer needs, modelling a stalled HDD. The
        // player-side scheduling call must still return immediately.
        let persistence = Arc::clone(&store.persistence);
        let held_storage = persistence.lock().unwrap();
        let started = std::time::Instant::now();
        assert!(store.save_in_background_and_take_error().is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(store.background_pending);

        // A newer playback position remains dirty instead of spawning another
        // writer or being lost behind the in-flight snapshot.
        store.record_open(second);
        store.record_position(second, 42.0, 100.0);
        assert!(store.dirty);
        drop(held_storage);

        store.save_if_dirty().unwrap();
        let loaded = SessionStore::load_from(&path).unwrap();
        assert_eq!(loaded.recent(), &[second.to_owned(), first.to_owned()]);
        assert_eq!(loaded.resume_position(second, 100.0), Some(42.0));
    }

    #[test]
    fn failed_session_save_is_reported_once_and_can_be_retried() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let lock_path = files.path("playback-state.txt.lock");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(Path::new("movie.mkv"));
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(lock_path)
            .unwrap();
        lock.try_lock().unwrap();
        assert!(store.save_and_take_error().is_some());
        assert!(store.dirty);
        assert!(store.save_and_take_error().is_none());
        drop(lock);
        assert!(store.save_and_take_error().is_none());
        assert!(!store.dirty);
        assert_eq!(
            SessionStore::load_from(&path).unwrap().recent(),
            &[PathBuf::from("movie.mkv")]
        );
    }

    #[test]
    fn newer_history_schema_is_not_silently_reset() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        std::fs::write(&path, "OXIDE_PLAYBACK_STATE_V3\n").unwrap();
        assert_eq!(
            SessionStore::load_from(&path).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn recent_files_are_unique_and_newest_first() {
        let mut store = SessionStore::default();
        store.record_open(Path::new("one.mp4"));
        store.record_open(Path::new("two.mp4"));
        store.record_open(Path::new("one.mp4"));
        assert_eq!(
            store.recent,
            [PathBuf::from("one.mp4"), PathBuf::from("two.mp4")]
        );
    }

    #[test]
    fn resume_position_ignores_opening_and_near_end_positions() {
        let mut store = SessionStore::default();
        let path = Path::new("movie.mkv");
        store.record_position(path, 3.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), None);

        store.record_position(path, 45.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), Some(45.0));

        store.record_position(path, 95.0, 100.0);
        assert_eq!(store.resume_position(path, 100.0), None);
    }

    #[test]
    fn clearing_playback_data_forgets_history_positions_and_recovery_copy() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let track = Path::new("Музыка/мой трек.flac");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(track);
        store.record_position(track, 42.5, 100.0);
        store.save_if_dirty().unwrap();

        store.clear_playback_data().unwrap();

        assert!(store.recent().is_empty());
        assert_eq!(store.resume_position(track, 100.0), None);
        assert!(!store.dirty);
        let loaded = SessionStore::load_from(&path).unwrap();
        assert!(loaded.recent().is_empty());
        assert_eq!(loaded.resume_position(track, 100.0), None);

        std::fs::write(&path, b"broken").unwrap();
        let mut recovered = SessionStore::load_from(&path).unwrap();
        assert!(recovered.recent().is_empty());
        assert_eq!(recovered.resume_position(track, 100.0), None);
        assert!(recovered.take_storage_notice().is_some());
    }

    #[test]
    fn failed_clear_retry_still_forgets_the_recovery_copy() {
        let files = TestDirectory::new();
        let path = files.path("playback-state.txt");
        let lock_path = files.path("playback-state.txt.lock");
        let track = Path::new("movie.mkv");
        let mut store = SessionStore::load_from(&path).unwrap();
        store.record_open(track);
        store.record_position(track, 30.0, 100.0);
        store.save_if_dirty().unwrap();

        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open(lock_path)
            .unwrap();
        lock.try_lock().unwrap();
        assert!(store.clear_playback_data().is_err());
        assert!(store.dirty);
        assert!(store.forget_previous_pending);
        drop(lock);

        assert!(store.save_and_take_error().is_none());
        assert!(!store.dirty);
        assert!(!store.forget_previous_pending);
        std::fs::write(&path, b"broken").unwrap();
        let recovered = SessionStore::load_from(&path).unwrap();
        assert!(recovered.recent().is_empty());
        assert_eq!(recovered.resume_position(track, 100.0), None);
    }
}
