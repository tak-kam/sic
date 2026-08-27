//! The disassembler.
//!
//! Its output is what `sic disasm` prints, and it is also how the bytecode
//! tests state their expectations, so it aims to be readable and stable rather
//! than clever.

use crate::inst::{Form, Inst, Op};
use crate::program::*;

pub fn disassemble(p: &Program) -> String {
    let mut out = String::new();
    if !p.debug.sources.is_empty() {
        out.push_str(&format!("; source: {}\n", p.debug.sources.join(", ")));
    }

    out.push_str("constants:\n");
    if p.consts.is_empty() {
        out.push_str("  (none)\n");
    }
    for (i, c) in p.consts.iter().enumerate() {
        out.push_str(&format!("  k{i} = {}\n", const_str(c)));
    }

    out.push_str("capabilities:\n");
    if p.caps.is_empty() {
        out.push_str("  (none)\n");
    }
    for (i, c) in p.caps.iter().enumerate() {
        let params: Vec<String> = c.params.iter().map(|t| p.type_name(*t)).collect();
        let ret = p.type_name(c.ret_type);
        out.push_str(&format!(
            "  c{i} = {}({}) -> {ret}  {} {:?}\n",
            c.name,
            params.join(", "),
            c.kind.name(),
            c.constraints
        ));
    }

    for f in &p.funcs {
        let ret = p.type_name(f.ret_type);
        out.push_str(&format!(
            "\nfn {}/{} -> {ret}  regs={}\n",
            f.name,
            f.param_count(),
            f.reg_count
        ));
        for i in 0..f.code_len {
            let pc = f.code_off + i;
            let Some(inst) = p.code.get(pc as usize) else {
                out.push_str(&format!("  {pc:04}  <past the end of the code section>\n"));
                break;
            };
            out.push_str(&format!("  {pc:04}  {}\n", inst_str(p, pc, *inst)));
        }
    }
    out
}

fn const_str(c: &Const) -> String {
    match c {
        Const::Unit => "unit".into(),
        Const::Bool(v) => format!("{v}"),
        Const::I64(v) => format!("{v}"),
        Const::F64(v) => format!("{v:?}"),
        Const::Str(s) => format!("{s:?}"),
        Const::EmptyList(index) => format!("[] : t{index}"),
    }
}

/// Formats one instruction, with the constant it loads or the function it calls
/// spelled out in a trailing comment.
fn inst_str(p: &Program, pc: u32, inst: Inst) -> String {
    let Some(op) = inst.op() else {
        return format!("<unknown opcode {}>", inst.raw_op());
    };
    let operands = match op.form() {
        Form::ABx => format!("r{}, k{}", inst.a(), inst.bx()),
        Form::AsBx => match op {
            Op::Jump => format!("{:+}  ; -> {:04}", inst.sbx(), target(pc, inst.sbx())),
            _ => format!(
                "r{}, {:+}  ; -> {:04}",
                inst.a(),
                inst.sbx(),
                target(pc, inst.sbx())
            ),
        },
        Form::ABC => match op {
            Op::Return | Op::Fail => format!("r{}", inst.a()),
            Op::Halt => String::new(),
            Op::Log => format!("{}, r{}", level_name(inst.a()), inst.b()),
            Op::Move | Op::Not => format!("r{}, r{}", inst.a(), inst.b()),
            Op::Call => format!("r{}, f{}, r{}", inst.a(), inst.b(), inst.c()),
            Op::CallCap => format!("r{}, c{}, r{}", inst.a(), inst.b(), inst.c()),
            Op::Spawn => format!("r{}, f{}, r{}", inst.a(), inst.b(), inst.c()),
            Op::Await | Op::Len => format!("r{}, r{}", inst.a(), inst.b()),
            Op::MakeObject => format!("r{}, t{}, r{}", inst.a(), inst.b(), inst.c()),
            Op::FromJson | Op::ToJson => format!("r{}, t{}, r{}", inst.a(), inst.b(), inst.c()),
            Op::GetField => format!("r{}, r{}, .{}", inst.a(), inst.b(), inst.c()),
            Op::MakeList => format!("r{}, r{}, {}", inst.a(), inst.b(), inst.c()),
            Op::GetIndex => format!("r{}, r{}, r{}", inst.a(), inst.b(), inst.c()),
            _ => format!("r{}, r{}, r{}", inst.a(), inst.b(), inst.c()),
        },
    };

    let mut line = format!("{:<12}{operands}", op.name());
    if op == Op::LoadConst {
        if let Some(c) = p.consts.get(inst.bx() as usize) {
            line.push_str(&format!("  ; {}", const_str(c)));
        }
    }
    if op == Op::Call {
        if let Some(f) = p.funcs.get(inst.b() as usize) {
            line.push_str(&format!("  ; {}/{}", f.name, f.param_count()));
        }
    }
    if op == Op::CallCap {
        if let Some(c) = p.caps.get(inst.b() as usize) {
            line.push_str(&format!("  ; {}", c.name));
        }
        if let Some(policy) = p.policy_at(pc) {
            if policy.attempts > 1 {
                line.push_str(&format!("  ; retry {}", policy.attempts));
            }
            if policy.timeout_ms > 0 {
                line.push_str(&format!("  ; timeout {}ms", policy.timeout_ms));
            }
        }
    }
    if op == Op::MakeObject || op == Op::FromJson || op == Op::ToJson {
        line.push_str(&format!("  ; {}", p.type_name(inst.b() as u32)));
    }
    if op == Op::Spawn {
        if let Some(f) = p.funcs.get(inst.b() as usize) {
            line.push_str(&format!("  ; {}/{}", f.name, f.param_count()));
        }
    }
    if let Some((line_no, col)) = p.debug.position(pc) {
        line.push_str(&format!("  ; {}:{}", line_no, col));
    }
    line
}

fn target(pc: u32, offset: i16) -> i64 {
    pc as i64 + 1 + offset as i64
}

/// The four levels, by the numbers the bytecode uses. Anything else is a file
/// the verifier refuses, and a disassembly says what it saw rather than
/// guessing.
fn level_name(code: u8) -> String {
    match code {
        0 => "debug".to_string(),
        1 => "info".to_string(),
        2 => "warn".to_string(),
        3 => "error".to_string(),
        other => format!("level{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disassembles_a_small_function() {
        let p = Program {
            consts: vec![Const::I64(10)],
            types: vec![TypeDesc::Int],
            funcs: vec![FuncDef {
                name: "main".into(),
                params: Vec::new(),
                reg_count: 1,
                ret_type: 0,
                code_off: 0,
                code_len: 2,
            }],
            caps: Vec::new(),
            code: vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::Return, 0, 0, 0),
            ],
            policies: Vec::new(),
            debug: DebugInfo::default(),
        };
        assert_eq!(
            disassemble(&p),
            "\
constants:
  k0 = 10
capabilities:
  (none)

fn main/0 -> Int  regs=1
  0000  LOAD_CONST  r0, k0  ; 10
  0001  RETURN      r0
"
        );
    }

    #[test]
    fn jumps_show_their_target() {
        let p = Program {
            types: vec![TypeDesc::Unit],
            funcs: vec![FuncDef {
                name: "f".into(),
                params: Vec::new(),
                reg_count: 1,
                ret_type: 0,
                code_off: 0,
                code_len: 2,
            }],
            code: vec![Inst::asbx(Op::Jump, 0, 1), Inst::asbx(Op::JumpIf, 0, -2)],
            ..Program::default()
        };
        let out = disassemble(&p);
        assert!(out.contains("JUMP        +1  ; -> 0002"), "{out}");
        assert!(out.contains("JUMP_IF     r0, -2  ; -> 0000"), "{out}");
    }
}
