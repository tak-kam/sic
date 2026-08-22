//! Instruction encoding.
//!
//! Every instruction is four bytes in one of three shapes:
//!
//! ```text
//! ABC : [ op:u8 ][ a:u8 ][ b:u8 ][ c:u8 ]
//! ABx : [ op:u8 ][ a:u8 ][     bx:u16   ]
//! AsBx: [ op:u8 ][ a:u8 ][    sbx:i16   ]
//! ```
//!
//! Fixed width turns "is this a valid instruction boundary" into arithmetic,
//! which is what keeps the verifier and the disassembler simple.

/// The opcodes of v0.1. Values are part of the file format and must not be
/// renumbered; new opcodes are appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    LoadConst = 0,
    Move = 1,

    AddI64 = 2,
    SubI64 = 3,
    MulI64 = 4,
    DivI64 = 5,
    RemI64 = 6,

    Eq = 7,
    Ne = 8,
    Lt = 9,
    Le = 10,
    Gt = 11,
    Ge = 12,
    Not = 13,

    Jump = 14,
    JumpIf = 15,
    JumpIfNot = 16,

    Call = 17,
    Return = 18,
    Fail = 19,
    Halt = 20,
}

impl Op {
    /// The last opcode that exists. Anything above it is invalid.
    pub const MAX: u8 = Op::Halt as u8;

    pub fn from_u8(v: u8) -> Option<Op> {
        use Op::*;
        Some(match v {
            0 => LoadConst,
            1 => Move,
            2 => AddI64,
            3 => SubI64,
            4 => MulI64,
            5 => DivI64,
            6 => RemI64,
            7 => Eq,
            8 => Ne,
            9 => Lt,
            10 => Le,
            11 => Gt,
            12 => Ge,
            13 => Not,
            14 => Jump,
            15 => JumpIf,
            16 => JumpIfNot,
            17 => Call,
            18 => Return,
            19 => Fail,
            20 => Halt,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        use Op::*;
        match self {
            LoadConst => "LOAD_CONST",
            Move => "MOVE",
            AddI64 => "ADD_I64",
            SubI64 => "SUB_I64",
            MulI64 => "MUL_I64",
            DivI64 => "DIV_I64",
            RemI64 => "REM_I64",
            Eq => "EQ",
            Ne => "NE",
            Lt => "LT",
            Le => "LE",
            Gt => "GT",
            Ge => "GE",
            Not => "NOT",
            Jump => "JUMP",
            JumpIf => "JUMP_IF",
            JumpIfNot => "JUMP_IF_NOT",
            Call => "CALL",
            Return => "RETURN",
            Fail => "FAIL",
            Halt => "HALT",
        }
    }

    /// Which operand shape the instruction uses.
    pub fn form(self) -> Form {
        use Op::*;
        match self {
            LoadConst => Form::ABx,
            Jump | JumpIf | JumpIfNot => Form::AsBx,
            _ => Form::ABC,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    ABC,
    ABx,
    AsBx,
}

/// One encoded instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inst(pub u32);

impl Inst {
    pub fn abc(op: Op, a: u8, b: u8, c: u8) -> Inst {
        Inst(op as u32 | (a as u32) << 8 | (b as u32) << 16 | (c as u32) << 24)
    }

    pub fn abx(op: Op, a: u8, bx: u16) -> Inst {
        Inst(op as u32 | (a as u32) << 8 | (bx as u32) << 16)
    }

    pub fn asbx(op: Op, a: u8, sbx: i16) -> Inst {
        Inst(op as u32 | (a as u32) << 8 | ((sbx as u16) as u32) << 16)
    }

    pub fn raw_op(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// The opcode, or `None` when the byte is not one. Only the verifier should
    /// ever see `None`; the VM runs verified code.
    pub fn op(self) -> Option<Op> {
        Op::from_u8(self.raw_op())
    }

    pub fn a(self) -> u8 {
        ((self.0 >> 8) & 0xFF) as u8
    }

    pub fn b(self) -> u8 {
        ((self.0 >> 16) & 0xFF) as u8
    }

    pub fn c(self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    pub fn bx(self) -> u16 {
        ((self.0 >> 16) & 0xFFFF) as u16
    }

    pub fn sbx(self) -> i16 {
        self.bx() as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_operands() {
        let i = Inst::abc(Op::AddI64, 1, 2, 3);
        assert_eq!(i.op(), Some(Op::AddI64));
        assert_eq!((i.a(), i.b(), i.c()), (1, 2, 3));

        let i = Inst::abx(Op::LoadConst, 7, 65535);
        assert_eq!(i.a(), 7);
        assert_eq!(i.bx(), 65535);

        for offset in [-32768i16, -1, 0, 1, 32767] {
            let i = Inst::asbx(Op::Jump, 0, offset);
            assert_eq!(i.sbx(), offset, "offset {offset}");
        }
    }

    #[test]
    fn every_opcode_round_trips_through_u8() {
        for raw in 0..=Op::MAX {
            let op = Op::from_u8(raw).expect("opcode below MAX must exist");
            assert_eq!(op as u8, raw);
            assert!(!op.name().is_empty());
        }
        assert_eq!(Op::from_u8(Op::MAX + 1), None);
    }
}
