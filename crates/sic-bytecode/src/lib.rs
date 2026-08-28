//! The bytecode: instruction set, in-memory program, file format, disassembler.
//!
//! This crate defines the format and nothing else. It does not know how a
//! program is produced (see `sic-compile`), whether it is safe to run (see
//! `sic-verify`), or how to run it (see `sic-vm`).

pub mod disasm;
pub mod file;
pub mod inst;
pub mod program;

pub use disasm::disassemble;
pub use file::{DecodeError, MAGIC, VERSION_MAJOR, VERSION_MINOR, decode, encode};
pub use inst::{Inst, Op};
pub use program::{
    CapDecl, CapKind, Const, DebugInfo, Field, FuncDef, PolicyEntry, Program, TypeDesc,
};
