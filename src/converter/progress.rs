use std::time::Duration;

const MAX_ERROR_LOG_BYTES: usize = 32 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024;

/// Incrementally decodes child-process output without retaining an unbounded
/// unterminated line. Invalid UTF-8 is replaced only at the process boundary.
#[derive(Default)]
pub(super) struct OutputLines {
    pending: Vec<u8>,
}

impl OutputLines {
    pub(super) fn feed(&mut self, bytes: &[u8], finished: bool) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                lines.push(
                    String::from_utf8_lossy(&self.pending)
                        .trim_end_matches('\r')
                        .to_owned(),
                );
                self.pending.clear();
            } else if self.pending.len() < MAX_LINE_BYTES {
                self.pending.push(byte);
            }
        }
        if finished && !self.pending.is_empty() {
            lines.push(String::from_utf8_lossy(&self.pending).into_owned());
            self.pending.clear();
        }
        lines
    }
}

pub(super) fn parse_progress_seconds(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" | "out_time_ms" => value
            .parse::<f64>()
            .ok()
            .map(|value| value / 1_000_000.0)
            .filter(|value| value.is_finite() && *value >= 0.0),
        "out_time" => parse_ffmpeg_timestamp(value),
        _ => None,
    }
}

pub(super) fn parse_progress_size(line: &str) -> Option<u64> {
    let (key, value) = line.trim().split_once('=')?;
    (key == "total_size")
        .then(|| value.parse::<u64>().ok())
        .flatten()
}

fn parse_ffmpeg_timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;
    let valid = parts.next().is_none()
        && hours.is_finite()
        && minutes.is_finite()
        && seconds.is_finite()
        && hours >= 0.0
        && (0.0..60.0).contains(&minutes)
        && (0.0..60.0).contains(&seconds);
    valid
        .then_some(hours * 3_600.0 + minutes * 60.0 + seconds)
        .filter(|value| value.is_finite())
}

pub(super) fn estimate_remaining(elapsed: Duration, fraction: f64) -> Option<f64> {
    if !(0.01..1.0).contains(&fraction) {
        return None;
    }
    Some((elapsed.as_secs_f64() * (1.0 - fraction) / fraction).max(0.0))
}

pub(super) fn append_error_line(log: &mut String, line: &str) {
    if log.len() >= MAX_ERROR_LOG_BYTES {
        return;
    }
    let remaining = MAX_ERROR_LOG_BYTES - log.len() - 1;
    let slice_end = line
        .char_indices()
        .take_while(|(index, character)| *index + character.len_utf8() <= remaining)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    log.push_str(&line[..slice_end]);
    log.push('\n');
}

pub(super) fn last_error_lines(log: &str, count: usize) -> String {
    let lines = log.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_lines_bound_unterminated_input_and_flush_utf8_lossily() {
        let mut lines = OutputLines::default();
        assert!(
            lines
                .feed(&vec![b'x'; MAX_LINE_BYTES + 512], false)
                .is_empty()
        );
        let flushed = lines.feed(&[], true);
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].len(), MAX_LINE_BYTES);

        let mut invalid_utf8 = OutputLines::default();
        assert_eq!(invalid_utf8.feed(&[0xff], true), vec!["�"]);
    }

    #[test]
    fn error_log_limit_preserves_utf8_boundaries() {
        let mut log = String::new();
        let line = "я".repeat(MAX_ERROR_LOG_BYTES);
        append_error_line(&mut log, &line);
        assert!(log.len() <= MAX_ERROR_LOG_BYTES);
        assert!(log.is_char_boundary(log.len()));
        append_error_line(&mut log, "ignored");
        assert!(log.len() <= MAX_ERROR_LOG_BYTES);
    }
}
