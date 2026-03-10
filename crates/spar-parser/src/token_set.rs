use crate::syntax_kind::SyntaxKind;

/// An efficient bitset of [`SyntaxKind`] values, supporting up to 128 kinds.
///
/// Uses two `u64` bitmasks internally. Because `SyntaxKind` is `#[repr(u16)]`,
/// the discriminant is used directly as the bit index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSet {
    lo: u64,
    hi: u64,
}

impl TokenSet {
    /// The empty set.
    pub const EMPTY: TokenSet = TokenSet { lo: 0, hi: 0 };

    /// Create a `TokenSet` containing the given kinds.
    ///
    /// This is a `const fn` so it can be used in static/const contexts.
    pub const fn new(kinds: &[SyntaxKind]) -> TokenSet {
        let mut lo = 0u64;
        let mut hi = 0u64;
        let mut i = 0;
        while i < kinds.len() {
            let bit = kinds[i] as u16;
            if bit < 64 {
                lo |= 1u64 << (bit as u64);
            } else if bit < 128 {
                hi |= 1u64 << ((bit - 64) as u64);
            }
            // Kinds >= 128 are silently ignored; the parser should not need
            // that many token kinds in a recovery set.
            i += 1;
        }
        TokenSet { lo, hi }
    }

    /// Return the union of two token sets.
    pub const fn union(self, other: TokenSet) -> TokenSet {
        TokenSet {
            lo: self.lo | other.lo,
            hi: self.hi | other.hi,
        }
    }

    /// Check whether this set contains the given kind.
    pub const fn contains(self, kind: SyntaxKind) -> bool {
        let bit = kind as u16;
        if bit < 64 {
            self.lo & (1u64 << (bit as u64)) != 0
        } else if bit < 128 {
            self.hi & (1u64 << ((bit - 64) as u64)) != 0
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_contains_nothing() {
        assert!(!TokenSet::EMPTY.contains(SyntaxKind::IDENT));
        assert!(!TokenSet::EMPTY.contains(SyntaxKind::SEMICOLON));
    }

    #[test]
    fn singleton_set() {
        let set = TokenSet::new(&[SyntaxKind::SEMICOLON]);
        assert!(set.contains(SyntaxKind::SEMICOLON));
        assert!(!set.contains(SyntaxKind::COLON));
    }

    #[test]
    fn multiple_kinds() {
        let set = TokenSet::new(&[SyntaxKind::SEMICOLON, SyntaxKind::COLON, SyntaxKind::COMMA]);
        assert!(set.contains(SyntaxKind::SEMICOLON));
        assert!(set.contains(SyntaxKind::COLON));
        assert!(set.contains(SyntaxKind::COMMA));
        assert!(!set.contains(SyntaxKind::DOT));
    }

    #[test]
    fn union_of_sets() {
        let a = TokenSet::new(&[SyntaxKind::SEMICOLON]);
        let b = TokenSet::new(&[SyntaxKind::COLON]);
        let combined = a.union(b);
        assert!(combined.contains(SyntaxKind::SEMICOLON));
        assert!(combined.contains(SyntaxKind::COLON));
        assert!(!combined.contains(SyntaxKind::COMMA));
    }

    #[test]
    fn hi_bits() {
        // Keywords are further into the enum (discriminant >= 64 range).
        // Verify that kinds in the upper half of the bitset work correctly.
        let set = TokenSet::new(&[SyntaxKind::PACKAGE_KW, SyntaxKind::END_KW]);
        assert!(set.contains(SyntaxKind::PACKAGE_KW));
        assert!(set.contains(SyntaxKind::END_KW));
        assert!(!set.contains(SyntaxKind::IDENT));
    }
}
