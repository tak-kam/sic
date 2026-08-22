//! The in-memory form of a bytecode module.

use crate::inst::Inst;

// The kind of an effect means the same thing to the compiler, the verifier, the
// VM and the broker, so it is defined once in sic-core.
pub use sic_core::CapKind;

/// A decoded module. Producing one says nothing about whether it is safe to
/// run; that is what `sic-verify` decides.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub consts: Vec<Const>,
    /// Type descriptors, referenced by index from the function table.
    pub types: Vec<TypeTag>,
    pub funcs: Vec<FuncDef>,
    /// The capability manifest. Empty in v0.1; filled in phase 3.
    pub caps: Vec<CapDecl>,
    /// Every function's instructions, concatenated.
    pub code: Vec<Inst>,
    pub debug: DebugInfo,
}

impl Program {
    /// Looks up a function by name, which is how the CLI finds `main`.
    pub fn func_by_name(&self, name: &str) -> Option<u32> {
        self.funcs
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

impl Const {
    pub fn tag(&self) -> u8 {
        match self {
            Const::Unit => 0,
            Const::Bool(_) => 1,
            Const::I64(_) => 2,
            Const::F64(_) => 3,
            Const::Str(_) => 4,
        }
    }

    /// The type a constant has once loaded, used by the verifier.
    pub fn type_tag(&self) -> TypeTag {
        match self {
            Const::Unit => TypeTag::Unit,
            Const::Bool(_) => TypeTag::Bool,
            Const::I64(_) => TypeTag::Int,
            Const::F64(_) => TypeTag::Float,
            Const::Str(_) => TypeTag::Str,
        }
    }
}

/// The types the bytecode level distinguishes.
///
/// This is deliberately coarser than the source language: the verifier only has
/// to keep the VM's assumptions true, so it tracks representations rather than
/// the full type system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TypeTag {
    Unit = 0,
    Bool = 1,
    Int = 2,
    Float = 3,
    Str = 4,
}

impl TypeTag {
    pub fn from_u8(v: u8) -> Option<TypeTag> {
        Some(match v {
            0 => TypeTag::Unit,
            1 => TypeTag::Bool,
            2 => TypeTag::Int,
            3 => TypeTag::Float,
            4 => TypeTag::Str,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeTag::Unit => "Unit",
            TypeTag::Bool => "Bool",
            TypeTag::Int => "Int",
            TypeTag::Float => "Float",
            TypeTag::Str => "String",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    /// The type of each parameter, as an index into `Program::types`.
    ///
    /// The verifier needs these to know the state of the entry frame and to
    /// check the arguments at a call site, so they are part of the format
    /// rather than something a caller has to supply.
    pub params: Vec<u32>,
    /// How many registers the frame needs. The verifier checks that no
    /// instruction addresses a register beyond this.
    pub reg_count: u8,
    /// Index into `Program::types`.
    pub ret_type: u32,
    /// Index of the first instruction in `Program::code`.
    pub code_off: u32,
    /// Number of instructions.
    pub code_len: u32,
}

impl FuncDef {
    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    pub fn contains_pc(&self, pc: u32) -> bool {
        pc >= self.code_off && pc < self.code_off + self.code_len
    }
}

/// A capability the module needs, with the signature of the call.
///
/// The signature is in the file so that the verifier can check a `CALL_CAP`
/// without trusting whoever produced the bytecode, and so that `sic verify` can
/// report what a module may do with nothing executed.
#[derive(Debug, Clone)]
pub struct CapDecl {
    pub name: String,
    pub kind: CapKind,
    /// Constraints such as an absolute path or a sha256 pin.
    pub constraints: String,
    /// Parameter types, as indices into `Program::types`.
    pub params: Vec<u32>,
    /// Result type, as an index into `Program::types`.
    pub ret_type: u32,
}

/// Source mapping, so a runtime error or a trace can name a line of source.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    pub source_name: String,
    /// `(pc, line, col)`, sorted by pc. Only the instructions that start a new
    /// position are listed.
    pub lines: Vec<(u32, u32, u32)>,
}

impl DebugInfo {
    /// The source position of an instruction, if the table has one at or before
    /// it.
    pub fn position(&self, pc: u32) -> Option<(u32, u32)> {
        match self.lines.binary_search_by_key(&pc, |e| e.0) {
            Ok(i) => Some((self.lines[i].1, self.lines[i].2)),
            Err(0) => None,
            Err(i) => Some((self.lines[i - 1].1, self.lines[i - 1].2)),
        }
    }
}
