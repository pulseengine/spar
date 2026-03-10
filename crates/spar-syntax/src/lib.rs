pub mod ast;
mod language;
mod parsing;

pub use language::{AadlLanguage, SyntaxElement, SyntaxNode, SyntaxNodeChildren, SyntaxToken};
pub use parsing::{Parse, SyntaxError, parse};
pub use spar_parser::SyntaxKind;
