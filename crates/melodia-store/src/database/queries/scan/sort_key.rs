//! The natural sort key `tracks.sort_key` holds, beside the scan writes that fill it.

/// A natural sort key: lowercased, each run of digits zero-padded to eight, so `"Track 2"` reads
/// `"track 00000002"`.
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
