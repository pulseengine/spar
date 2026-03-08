use spar_parser::SyntaxKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AadlLanguage {}

impl rowan::Language for AadlLanguage {
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

pub type SyntaxNode = rowan::SyntaxNode<AadlLanguage>;
pub type SyntaxToken = rowan::SyntaxToken<AadlLanguage>;
pub type SyntaxElement = rowan::SyntaxElement<AadlLanguage>;
pub type SyntaxNodeChildren = rowan::SyntaxNodeChildren<AadlLanguage>;
