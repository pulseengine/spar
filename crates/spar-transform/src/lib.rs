//! Bidirectional transforms between AADL and external formats.
//!
//! This crate provides the ability to treat external format files (e.g., WIT)
//! as "virtual AADL files" by converting them into the same `ItemTree`
//! representation used by the rest of the spar toolchain. It also supports
//! generating external format text from AADL `ItemTree`s.

pub mod cargo_metadata;
pub mod rust_crate;
pub mod wac;
pub mod wac_parser;
pub mod wit;
pub mod wit_parser;
pub mod wrpc;

/// A bidirectional transform between AADL and another format.
pub trait Transform {
    /// The external format's parsed representation.
    type External;

    /// Parse the external format from text.
    fn parse_external(source: &str) -> Result<Self::External, Vec<String>>;

    /// Convert external representation to an AADL ItemTree.
    fn to_aadl(external: &Self::External) -> spar_hir_def::item_tree::ItemTree;

    /// Convert an AADL ItemTree to external format text.
    fn from_aadl(tree: &spar_hir_def::item_tree::ItemTree) -> String;
}
