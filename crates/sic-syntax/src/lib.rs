//! The syntax layer of sic: lexer, AST, and parser.
//!
//! This crate performs no I/O. It takes a source string and returns an AST plus
//! diagnostics.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod print;
pub mod token;

pub use ast::Module;
pub use lexer::tokenize;
pub use parser::parse;
