pub const UI_TEXT_BYTES: usize = 4 * 1024;
pub const PATH_TEXT_BYTES: usize = 128 * 1024;

pub fn bounded_text(value: &str, limit: usize) -> String {
    let value = value.trim();
    if value.len() <= limit {
        return value.to_owned();
    }
    const ELLIPSIS: &str = "…";
    if limit < ELLIPSIS.len() {
        return String::new();
    }
    let mut end = limit.saturating_sub(ELLIPSIS.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = String::with_capacity(limit);
    output.push_str(&value[..end]);
    output.push_str(ELLIPSIS);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_limit_preserves_utf8_and_marks_truncation() {
        assert_eq!(bounded_text("  title  ", 16), "title");
        let value = "я".repeat(10);
        let bounded = bounded_text(&value, 10);
        assert!(bounded.is_char_boundary(bounded.len()));
        assert!(bounded.len() <= 10);
        assert!(bounded.ends_with('…'));
        assert_eq!(bounded_text("long", 2), "");
    }
}
