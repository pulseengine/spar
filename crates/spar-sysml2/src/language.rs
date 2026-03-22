use crate::syntax_kind::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SysmlLanguage {}

impl rowan::Language for SysmlLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        assert!(
            (raw.0 as usize) < SyntaxKind::__LAST as usize,
            "raw SyntaxKind {} out of range",
            raw.0
        );
        // SAFETY: SyntaxKind is repr(u16) and we bounds-checked above.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<SysmlLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<SysmlLanguage>;
#[allow(dead_code)]
pub type SyntaxElement = rowan::SyntaxElement<SysmlLanguage>;
#[allow(dead_code)]
pub type SyntaxNodeChildren = rowan::SyntaxNodeChildren<SysmlLanguage>;
