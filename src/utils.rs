//! Common utility functions
use std::time::SystemTime;

/// Seconds since 1970.
#[must_use] pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

/// Check if a string contains only hex characters.
#[must_use] pub fn is_hex(s: &str) -> bool {
    s.chars().all(|x| char::is_ascii_hexdigit(&x))
}

/// Check if a string contains only lower-case hex chars.
#[must_use] pub fn is_lower_hex(s: &str) -> bool {
    s.chars().all(|x| {
        (char::is_ascii_lowercase(&x) || char::is_ascii_digit(&x)) && char::is_ascii_hexdigit(&x)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_hex() {
        let hexstr = "abcd0123";
        assert_eq!(is_lower_hex(hexstr), true);
    }
}
