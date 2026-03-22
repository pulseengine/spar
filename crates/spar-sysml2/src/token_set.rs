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
