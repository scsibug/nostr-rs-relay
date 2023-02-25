//! Common utility functions
use bech32::FromBase32;
use std::time::SystemTime;
use url::Url;

/// Seconds since 1970.
#[must_use]
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

/// Check if a string contains only hex characters.
#[must_use]
pub fn is_hex(s: &str) -> bool {
    s.chars().all(|x| char::is_ascii_hexdigit(&x))
}

/// Check if string is a nip19 string
pub fn is_nip19(s: &str) -> bool {
    s.starts_with("npub") || s.starts_with("note")
}

pub fn nip19_to_hex(s: &str) -> Result<String, bech32::Error> {
    let (_hrp, data, _checksum) = bech32::decode(s)?;
    let data = Vec::<u8>::from_base32(&data)?;
    Ok(hex::encode(data))
}

/// Check if a string contains only lower-case hex chars.
#[must_use]
pub fn is_lower_hex(s: &str) -> bool {
    s.chars().all(|x| {
        (char::is_ascii_lowercase(&x) || char::is_ascii_digit(&x)) && char::is_ascii_hexdigit(&x)
    })
}

pub fn host_str(url: &String) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_hex() {
        let hexstr = "abcd0123";
        assert_eq!(is_lower_hex(hexstr), true);
    }

    #[test]
    fn nip19() {
        let hexkey = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let nip19key = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";
        assert_eq!(is_nip19(hexkey), false);
        assert_eq!(is_nip19(nip19key), true);
    }

    #[test]
    fn nip19_hex() {
        let nip19key = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";
        let expected = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let got = nip19_to_hex(nip19key).unwrap();

        assert_eq!(expected, got);
    }
}
