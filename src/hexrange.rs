//! Utilities for searching hexadecimal
use crate::utils::is_hex;
use hex;

/// Types of hexadecimal queries.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone)]
pub enum HexSearch {
    // when no range is needed, exact 32-byte
    Exact(Vec<u8>),
    // lower (inclusive) and upper range (exclusive)
    Range(Vec<u8>, Vec<u8>),
    // lower bound only, upper bound is MAX inclusive
    LowerOnly(Vec<u8>),
}

/// Check if a string contains only f chars
fn is_all_fs(s: &str) -> bool {
    s.chars().all(|x| x == 'f' || x == 'F')
}

/// Find the next hex sequence greater than the argument.
#[must_use]
pub fn hex_range(s: &str) -> Option<HexSearch> {
    let mut hash_base = s.to_owned();
    if !is_hex(&hash_base) || hash_base.len() > 64 {
        return None;
    }
    if hash_base.len() == 64 {
        return Some(HexSearch::Exact(hex::decode(&hash_base).ok()?));
    }
    // if s is odd, add a zero
    let mut odd = hash_base.len() % 2 != 0;
    if odd {
        // extend the string to make it even
        hash_base.push('0');
    }
    let base = hex::decode(hash_base).ok()?;
    // check for all ff's
    if is_all_fs(s) {
        // there is no higher bound, we only want to search for blobs greater than this.
        return Some(HexSearch::LowerOnly(base));
    }

    // return a range
    let mut upper = base.clone();
    let mut byte_len = upper.len();

    // for odd strings, we made them longer, but we want to increment the upper char (+16).
    // we know we can do this without overflowing because we explicitly set the bottom half to 0's.
    while byte_len > 0 {
        byte_len -= 1;
        // check if byte can be incremented, or if we need to carry.
        let b = upper[byte_len];
        if b == u8::MAX {
            // reset and carry
            upper[byte_len] = 0;
        } else if odd {
            // check if first char in this byte is NOT 'f'
            if b < 240 {
                // bump up the first character in this byte
                upper[byte_len] = b + 16;
                // increment done, stop iterating through the vec
                break;
            }
            // if it is 'f', reset the byte to 0 and do a carry
            // reset and carry
            upper[byte_len] = 0;
            // done with odd logic, so don't repeat this
            odd = false;
        } else {
            // bump up the first character in this byte
            upper[byte_len] = b + 1;
            // increment done, stop iterating
            break;
        }
    }
    Some(HexSearch::Range(base, upper))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn hex_range_exact() -> Result<()> {
        let hex = "abcdef00abcdef00abcdef00abcdef00abcdef00abcdef00abcdef00abcdef00";
        let r = hex_range(hex);
        assert_eq!(
            r,
            Some(HexSearch::Exact(hex::decode(hex).expect("invalid hex")))
        );
        Ok(())
    }
    #[test]
    fn hex_full_range() -> Result<()> {
        let hex = "aaaa";
        let hex_upper = "aaab";
        let r = hex_range(hex);
        assert_eq!(
            r,
            Some(HexSearch::Range(
                hex::decode(hex).expect("invalid hex"),
                hex::decode(hex_upper).expect("invalid hex")
            ))
        );
        Ok(())
    }

    #[test]
    fn hex_full_range_odd() -> Result<()> {
        let r = hex_range("abc");
        assert_eq!(
            r,
            Some(HexSearch::Range(
                hex::decode("abc0").expect("invalid hex"),
                hex::decode("abd0").expect("invalid hex")
            ))
        );
        Ok(())
    }

    #[test]
    fn hex_full_range_odd_end_f() -> Result<()> {
        let r = hex_range("abf");
        assert_eq!(
            r,
            Some(HexSearch::Range(
                hex::decode("abf0").expect("invalid hex"),
                hex::decode("ac00").expect("invalid hex")
            ))
        );
        Ok(())
    }

    #[test]
    fn hex_no_upper() -> Result<()> {
        let r = hex_range("ffff");
        assert_eq!(
            r,
            Some(HexSearch::LowerOnly(
                hex::decode("ffff").expect("invalid hex")
            ))
        );
        Ok(())
    }

    #[test]
    fn hex_no_upper_odd() -> Result<()> {
        let r = hex_range("fff");
        assert_eq!(
            r,
            Some(HexSearch::LowerOnly(
                hex::decode("fff0").expect("invalid hex")
            ))
        );
        Ok(())
    }
}
