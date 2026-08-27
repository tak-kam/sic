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

/// Declares the instruction set once.
///
/// Four things follow from the same row: the variant, its number, the spelling
/// the disassembler prints, and which operand shape it uses. They used to be
/// three full lists and a fourth that named only the exceptions, and only two
/// of the four were checked against each other. A spelling copied onto two
/// opcodes passed, and a disassembly then quietly named the wrong instruction.
///
/// A macro is worth it here because the thing being written is a table and
/// nothing else: there is no control flow to hide, and the operand shape stops
/// being something a reader has to infer from an absence. It is not worth it
/// for the four crates that `match` on `Op` to do work - emitting, verifying,
/// executing, disassembling are four different jobs, and the compiler already
/// makes each one account for a new opcode.
macro_rules! opcodes {
    ($( $(#[$doc:meta])* $variant:ident = $value:literal, $name:literal, $form:ident; )*) => {
        /// The opcodes of v0.1. Values are part of the file format and must not
        /// be renumbered; new opcodes are appended.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Op {
            $( $(#[$doc])* $variant = $value, )*
        }

        impl Op {
            /// The last opcode that exists. Anything above it is invalid.
            ///
            /// The largest number answers it rather than the count, because
            /// the numbers are allowed to have a gap in them. A number is part
            /// of the file format, so two opcodes added on two branches have to
            /// pick different ones before either is merged, and the branch that
            /// lands second must not renumber the first. A gap is what that
            /// costs; `from_u8` answers `None` for one, and the verifier
            /// refuses it as an unknown opcode like any other byte.
            pub const MAX: u8 = {
                let values = [$($value as u8),*];
                let mut i = 0;
                let mut max = 0u8;
                while i < values.len() {
                    if values[i] > max {
                        max = values[i];
                    }
                    i += 1;
                }
                max
            };

            pub fn from_u8(v: u8) -> Option<Op> {
                Some(match v {
                    $( $value => Op::$variant, )*
                    _ => return None,
                })
            }

            pub fn name(self) -> &'static str {
                match self {
                    $( Op::$variant => $name, )*
                }
            }

            /// Which operand shape the instruction uses.
            pub fn form(self) -> Form {
                match self {
                    $( Op::$variant => Form::$form, )*
                }
            }
        }
    };
}

opcodes! {
    LoadConst = 0, "LOAD_CONST", ABx;
    Move = 1, "MOVE", ABC;

    AddI64 = 2, "ADD_I64", ABC;
    SubI64 = 3, "SUB_I64", ABC;
    MulI64 = 4, "MUL_I64", ABC;
    DivI64 = 5, "DIV_I64", ABC;
    RemI64 = 6, "REM_I64", ABC;

    Eq = 7, "EQ", ABC;
    Ne = 8, "NE", ABC;
    Lt = 9, "LT", ABC;
    Le = 10, "LE", ABC;
    Gt = 11, "GT", ABC;
    Ge = 12, "GE", ABC;
    Not = 13, "NOT", ABC;

    Jump = 14, "JUMP", AsBx;
    JumpIf = 15, "JUMP_IF", AsBx;
    JumpIfNot = 16, "JUMP_IF_NOT", AsBx;

    Call = 17, "CALL", ABC;
    /// The only instruction that reaches outside the VM.
    CallCap = 18, "CALL_CAP", ABC;
    /// Starts a task. Same shape as CALL.
    Spawn = 19, "SPAWN", ABC;
    /// Waits for a task and takes its result.
    Await = 20, "AWAIT", ABC;
    MakeObject = 21, "MAKE_OBJECT", ABC;
    GetField = 22, "GET_FIELD", ABC;
    MakeList = 23, "MAKE_LIST", ABC;
    GetIndex = 24, "GET_INDEX", ABC;
    Len = 25, "LEN", ABC;
    /// Parses and validates a document against a type. The only way a value
    /// enters a run from text.
    FromJson = 26, "FROM_JSON", ABC;
    Return = 27, "RETURN", ABC;
    Fail = 28, "FAIL", ABC;
    Halt = 29, "HALT", ABC;
    /// What the program has to say about itself. `a` is the level, `b` the
    /// register holding the message. The only instruction whose whole effect
    /// is an entry in the journal.
    Log = 30, "LOG", ABC;
    /// Whether one string occurs anywhere in another.
    Contains = 31, "CONTAINS", ABC;
    /// Whether one string begins another. Not a special case of `CONTAINS`:
    /// a grant is about a prefix, and a match in the middle is a different
    /// answer to a question about one.
    StartsWith = 32, "STARTS_WITH", ABC;
    /// Joins two strings into a third. The only instruction that allocates
    /// without a capability having been called, which is why the VM charges
    /// fuel for it by the byte - see `docs/design/v0.1.md` §6.
    Concat = 33, "CONCAT", ABC;
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

    /// A number that decodes decodes as itself, and `MAX` is the last one that
    /// decodes at all.
    ///
    /// The loop skips the numbers no opcode claims rather than requiring every
    /// one below `MAX` to exist. That is not laxness: a number no opcode claims
    /// decodes to `None`, which the verifier reports as an unknown opcode like
    /// any other byte. Requiring no gaps would mean the branch that merges
    /// second has to renumber the opcode the first one shipped, and a number is
    /// part of the file format.
    #[test]
    fn every_opcode_round_trips_through_u8() {
        for raw in 0..=Op::MAX {
            let Some(op) = Op::from_u8(raw) else {
                continue;
            };
            assert_eq!(op as u8, raw);
            assert!(!op.name().is_empty());
        }
        assert!(Op::from_u8(Op::MAX).is_some(), "MAX must be an opcode");
        assert_eq!(Op::from_u8(Op::MAX + 1), None);
    }

    /// A spelling belongs to one opcode. Two sharing one would make a
    /// disassembly name the wrong instruction, and it would still read as a
    /// valid disassembly.
    #[test]
    fn no_two_opcodes_are_spelled_the_same() {
        let mut seen: Vec<&'static str> = Vec::new();
        for raw in 0..=Op::MAX {
            let Some(name) = Op::from_u8(raw).map(Op::name) else {
                continue;
            };
            assert!(
                !seen.contains(&name),
                "{name} is the spelling of more than one opcode"
            );
            seen.push(name);
        }
    }

    /// The form is what the disassembler reads the operands with, so an
    /// opcode whose row says the wrong one prints the wrong numbers. Every
    /// form appears, which is what makes the table's third column worth
    /// having rather than an assumption that everything is ABC.
    #[test]
    fn every_operand_shape_is_used() {
        let mut abc = 0;
        let mut abx = 0;
        let mut asbx = 0;
        let mut total = 0;
        for raw in 0..=Op::MAX {
            let Some(op) = Op::from_u8(raw) else {
                continue;
            };
            total += 1;
            match op.form() {
                Form::ABC => abc += 1,
                Form::ABx => abx += 1,
                Form::AsBx => asbx += 1,
            }
        }
        assert_eq!(
            (abx, asbx),
            (1, 3),
            "LOAD_CONST is ABx, the three jumps AsBx"
        );
        assert_eq!(abc, total - 4);
    }
}
