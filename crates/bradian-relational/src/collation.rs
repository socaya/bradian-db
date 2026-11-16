//! ADR 0103 — Collation primitive.
//!
//! Minimal UTF-8 comparison + sort-key abstraction. Two
//! collations are supported today:
//!
//! - **Binary** — bitwise `str.cmp()`. The default; matches
//!   every existing ORDER BY / UNIQUE comparison.
//! - **CaseInsensitive** — ASCII case-folded compare; UTF-8
//!   bytes above 0x7F compare bytewise. NOT full Unicode UCA;
//!   see §Non-goals.
//!
//! The intent is to give operators a stable way to say "sort
//! this column case-insensitively" or key a unique index that
//! treats `"Alice"` and `"alice"` as the same. Full Unicode
//! locale-aware collation requires ICU; that's a future ADR.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Collation {
    #[default]
    Binary,
    /// ASCII case-insensitive. Non-ASCII bytes compare
    /// bytewise. Not full Unicode case folding.
    CaseInsensitive,
}

impl Collation {
    /// Compare two UTF-8 strings under this collation.
    pub fn compare(&self, a: &str, b: &str) -> std::cmp::Ordering {
        match self {
            Collation::Binary => a.cmp(b),
            Collation::CaseInsensitive => {
                // Byte-by-byte ASCII case fold; non-ASCII
                // bytes compare raw. Length ties: shorter
                // sorts first.
                let bytes_a = a.as_bytes();
                let bytes_b = b.as_bytes();
                let common = bytes_a.len().min(bytes_b.len());
                for i in 0..common {
                    let ca = fold_ascii(bytes_a[i]);
                    let cb = fold_ascii(bytes_b[i]);
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => continue,
                        other => return other,
                    }
                }
                bytes_a.len().cmp(&bytes_b.len())
            }
        }
    }

    /// Produce a comparable sort key. Useful when the caller
    /// wants to cache sort orderings: `sort_by_key(|row|
    /// coll.sort_key(&row.name))`.
    pub fn sort_key(&self, s: &str) -> Vec<u8> {
        match self {
            Collation::Binary => s.as_bytes().to_vec(),
            Collation::CaseInsensitive =>
                s.as_bytes().iter().map(|b| fold_ascii(*b)).collect(),
        }
    }

    /// Equality under the collation. Convenience wrapper.
    pub fn eq(&self, a: &str, b: &str) -> bool {
        self.compare(a, b) == std::cmp::Ordering::Equal
    }
}

#[inline]
fn fold_ascii(b: u8) -> u8 {
    if b.is_ascii_uppercase() { b + 32 } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn binary_is_the_default() {
        assert_eq!(Collation::default(), Collation::Binary);
    }

    #[test]
    fn binary_cmp_matches_str_cmp() {
        assert_eq!(Collation::Binary.compare("Alice", "alice"), "Alice".cmp("alice"));
        assert_eq!(Collation::Binary.compare("abc", "abc"), Ordering::Equal);
    }

    #[test]
    fn case_insensitive_treats_ascii_case_as_equal() {
        let c = Collation::CaseInsensitive;
        assert_eq!(c.compare("Alice", "alice"), Ordering::Equal);
        assert!(c.eq("ALICE", "Alice"));
        assert_eq!(c.compare("alpha", "beta"), Ordering::Less);
    }

    #[test]
    fn sort_key_round_trips_ordering() {
        let c = Collation::CaseInsensitive;
        let mut v = vec!["Charlie".to_string(), "alice".to_string(), "Bob".to_string()];
        v.sort_by_key(|s| c.sort_key(s));
        assert_eq!(v, vec!["alice", "Bob", "Charlie"]);
    }

    #[test]
    fn non_ascii_bytes_compare_bytewise_under_case_insensitive() {
        let c = Collation::CaseInsensitive;
        // é = 0xC3 0xA9; É = 0xC3 0x89. ASCII fold leaves
        // both alone. They compare bytewise.
        assert_ne!(c.compare("éclair", "Éclair"), Ordering::Equal,
            "non-ASCII case folding requires Unicode UCA, not shipped");
    }

    #[test]
    fn length_ties_shorter_first() {
        let c = Collation::CaseInsensitive;
        assert_eq!(c.compare("alpha", "alphabet"), Ordering::Less);
        assert_eq!(c.compare("alphabet", "alpha"), Ordering::Greater);
    }
}
