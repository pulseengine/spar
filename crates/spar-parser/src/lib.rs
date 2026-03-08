//! AADL parser — tree-agnostic recursive descent with error recovery.
//!
//! This crate implements the core parsing infrastructure for the `spar` AADL
//! toolchain. It produces a flat stream of [`Event`]s that a tree builder
//! (in `spar-syntax`) converts into a lossless concrete syntax tree.
//!
//! # Architecture
//!
//! * [`syntax_kind`] — every token and node kind in AADL.
//! * [`event`] — the event types produced by the parser.
//! * [`token_set`] — efficient bitset for recovery sets.
//! * [`marker`] — marker/completed-marker system for building the event stream.
//! * [`parser`] — the `Parser` struct that grammar functions call into.

pub mod syntax_kind;
pub mod event;
pub mod token_set;
pub mod marker;
pub mod parser;

pub mod lexer;
pub mod grammar;

pub use syntax_kind::SyntaxKind;
