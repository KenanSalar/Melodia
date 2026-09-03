//! Natural sort key precomputation. Lives next to the scan code because
//! `insert_track` and `update_track_metadata` both write this column.

/// Precompute a natural sort key by zero-padding numeric segments.
/// e.g. "Track 2" → "track 00000002", "10 Songs" → "00000010 songs"
pub fn to_natural_sort_key(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut result = String::with_capacity(lower.len() + 16);
    let mut chars = lower.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut num = String::new();
            num.push(c);
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() {
                    num.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            // Pad to 8 digits
            for _ in 0..(8usize.saturating_sub(num.len())) {
                result.push('0');
            }
            result.push_str(&num);
        } else {
            result.push(c);
        }
    }

    result
}
