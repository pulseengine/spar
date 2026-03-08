mod language;
mod parsing;
pub mod ast;

pub use language::{AadlLanguage, SyntaxElement, SyntaxNode, SyntaxNodeChildren, SyntaxToken};
pub use parsing::{parse, Parse, SyntaxError};
pub use spar_parser::SyntaxKind;
