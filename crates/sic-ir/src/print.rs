//! A textual dump of the HIR, used by tests and by `sic` when asked to show it.

use sic_syntax::ast::{BinOp, UnOp};

use crate::hir::*;

pub fn dump(hir: &Hir) -> String {
    let mut out = String::new();
    if !hir.caps.is_empty() {
        out.push_str("capabilities:\n");
        for (i, c) in hir.caps.iter().enumerate() {
            out.push_str(&format!("  c{i} = {} {:?}\n", c.name, c.constraint));
        }
    }
    if !hir.consts.is_empty() {
        out.push_str("consts:\n");
        for (i, c) in hir.consts.iter().enumerate() {
            out.push_str(&format!("  k{i} = {}\n", const_str(c)));
        }
    }
    for f in &hir.funcs {
        out.push_str(&format!("\nfn {}/{}:\n", f.name, f.params.len()));
        for block in &f.blocks {
            out.push_str(&format!("  bb{}:\n", block.id.0));
            for inst in &block.insts {
                out.push_str(&format!("    {}\n", inst_str(&inst.kind)));
            }
            out.push_str(&format!("    {}\n", term_str(&block.term.kind)));
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
    }
}

fn inst_str(kind: &InstKind) -> String {
    match kind {
        InstKind::Const { dst, k } => format!("%{} = const k{}", dst.0, k.0),
        InstKind::Move { dst, src } => format!("%{} = move %{}", dst.0, src.0),
        InstKind::Approve { dst, src } => format!("%{} = approve %{}", dst.0, src.0),
        InstKind::Un { dst, op, x } => format!("%{} = {} %{}", dst.0, un_name(*op), x.0),
        InstKind::Bin { dst, op, l, r } => {
            format!("%{} = {} %{} %{}", dst.0, bin_name(*op), l.0, r.0)
        }
        InstKind::Call { dst, func, args } => {
            format!("%{} = call f{}({})", dst.0, func.0, locals(args))
        }
        InstKind::CallCap { dst, cap, args, .. } => {
            format!("%{} = call_cap c{}({})", dst.0, cap.0, locals(args))
        }
        InstKind::Spawn { dst, func, args } => {
            format!("%{} = spawn f{}({})", dst.0, func.0, locals(args))
        }
        InstKind::Await { dst, task } => format!("%{} = await %{}", dst.0, task.0),
        InstKind::MakeObject { dst, fields, .. } => {
            format!("%{} = object({})", dst.0, locals(fields))
        }
        InstKind::GetField { dst, base, index } => {
            format!("%{} = field %{} .{index}", dst.0, base.0)
        }
        InstKind::GetOpt { dst, base, index } => {
            format!("%{} = field? %{} .{index}", dst.0, base.0)
        }
        InstKind::HasOpt { dst, base, index } => {
            format!("%{} = has %{} .{index}", dst.0, base.0)
        }
        InstKind::MakeList { dst, elements, .. } => {
            format!("%{} = list({})", dst.0, locals(elements))
        }
        InstKind::GetIndex { dst, base, index } => {
            format!("%{} = index %{} %{}", dst.0, base.0, index.0)
        }
        InstKind::Len { dst, src } => format!("%{} = len %{}", dst.0, src.0),
        InstKind::Contains { dst, s, sub } => {
            format!("%{} = contains %{} %{}", dst.0, s.0, sub.0)
        }
        InstKind::StartsWith { dst, s, prefix } => {
            format!("%{} = starts_with %{} %{}", dst.0, s.0, prefix.0)
        }
        InstKind::FromJson { dst, src, .. } => format!("%{} = from_json %{}", dst.0, src.0),
        InstKind::ToJson { dst, src, .. } => format!("%{} = to_json %{}", dst.0, src.0),
        InstKind::Log { level, msg } => format!("log {} %{}", level.name(), msg.0),
    }
}

fn term_str(term: &Term) -> String {
    match term {
        Term::Jump(bb) => format!("jump bb{}", bb.0),
        Term::Branch {
            cond,
            then_bb,
            else_bb,
        } => format!("branch %{} bb{} bb{}", cond.0, then_bb.0, else_bb.0),
        Term::Return(Some(v)) => format!("return %{}", v.0),
        Term::Return(None) => "return".into(),
        Term::Fail(v) => format!("fail %{}", v.0),
    }
}

fn locals(ids: &[sic_core::LocalId]) -> String {
    ids.iter()
        .map(|l| format!("%{}", l.0))
        .collect::<Vec<_>>()
        .join(", ")
}

fn un_name(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "neg",
        UnOp::Not => "not",
    }
}

fn bin_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Rem => "rem",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Le => "le",
        BinOp::Gt => "gt",
        BinOp::Ge => "ge",
        BinOp::And => "and",
        BinOp::Or => "or",
    }
}
