//! The `.sicb` file format.
//!
//! ```text
//! MAGIC "SICB" | VERSION major,minor | FLAGS | SECTION_COUNT
//! SECTION_TABLE [ kind, offset, length ] * n
//! section bodies
//! ```
//!
//! Everything is little-endian and every variable-length item is preceded by
//! its length. Decoding performs the structural checks that must hold before a
//! `Program` even exists; the semantic checks live in `sic-verify`.

use sic_core::bin::{Reader, Writer};

use crate::inst::Inst;
use crate::program::*;

pub const MAGIC: [u8; 4] = *b"SICB";
pub const VERSION_MAJOR: u16 = 0;
/// Bumped from 3 for conversations: a policy entry now says which conversation
/// a call belongs to, and a reader that stopped after the budget would take the
/// next entry's `pc` for it.
/// Bumped from 4 for the two bounds an agent with tools needs, in the same
/// entry and for the same reason.
/// Bumped from 5 for `repeatable`: a manifest entry now says whether the effect
/// may be performed twice, and a reader that did not know would take the flag
/// for the start of the parameter list.
/// Bumped from 6 for `delegable`, for exactly the same reason: a second flag in
/// the same place, and a reader that stopped after the first would take it for
/// the parameter count.
/// Bumped from 7 for `in` and `env`: a manifest entry now says what directory
/// and what environment a child gets, and a reader that stopped after the flags
/// would take the directory's length for the parameter count.
/// Bumped from 8 for an open record: a record descriptor now carries whether
/// `from_json` may ignore an undeclared field, and a reader that did not know
/// would take that flag for the field count and then read a type section that
/// happens to decode. A new instruction has twice been judged not to need a
/// bump, because an old reader meets it as an unknown opcode; a changed section
/// layout is the other case, where the file decodes into something else.
/// Bumped from 9 for `answers`: a manifest entry now carries the shape a grant
/// says the program answers in, one byte after the pin, and a reader that did
/// not know would take it for the `repeatable` flag and every field after it
/// for the one before. Two layouts must not share a number even when the
/// number they would share was never released - that is the whole of what a
/// version is for, and 9 is one commit old rather than free.
/// Bumped from 10 for an optional field: every field of a record descriptor now
/// carries a byte saying whether a document may leave it out, and a reader that
/// did not know would take the first field's byte for the second field's name
/// length. This is the same case as 9, one level further in - and note that the
/// two new opcodes it arrived with are not: an old reader meets those as
/// unknown instructions and says so.
/// Bumped from 11 for the allowance a budget draws on: a policy entry now says
/// which count a site spends from, one word after the budget, and a reader that
/// did not know would take it for the conversation and every field after it for
/// the one before. The same case as 9 and 11, in the policy table this time -
/// and the reason for the field is in `agents.md` §6: a budget is written on an
/// agent, so the sites it lowers to have to share one count.
pub const VERSION_MINOR: u16 = 13;

pub mod section {
    pub const CONSTANTS: u32 = 1;
    pub const TYPES: u32 = 2;
    pub const FUNCTIONS: u32 = 3;
    pub const CAPABILITIES: u32 = 4;
    pub const CODE: u32 = 5;
    pub const DEBUG: u32 = 6;
    pub const SIGNATURE: u32 = 7;
    pub const POLICIES: u32 = 8;
}

/// An upper bound on the section table, so a corrupt count cannot make the
/// decoder allocate.
const MAX_SECTIONS: u32 = 64;

/// Decoding failures are the shared binary-format error: everything that can go
/// wrong here is "these bytes are not that".
pub type DecodeError = sic_core::BinError;

type Result<T> = std::result::Result<T, DecodeError>;

// ---------------------------------------------------------------- encoding

/// What a module is called, once, so that everything naming one agrees.
///
/// The digest is of the encoded module rather than of whatever file it may have
/// come from, which is what lets a program compiled in memory be named at all -
/// and what makes the name the same whether a run started from `.sic` or
/// `.sicb`. `sic plan` prints it, a checkpoint carries it and refuses to
/// restore against anything else, and a journal records it at `RunStarted`.
/// Those are three readers of one claim, and they were three copies of this
/// line until one of them needed to be somewhere the other two could not reach.
pub fn digest(p: &Program) -> sic_core::Digest {
    sic_core::Digest::of(&encode(p))
}

pub fn encode(p: &Program) -> Vec<u8> {
    let mut sections: Vec<(u32, Vec<u8>)> = Vec::new();

    let mut w = Writer::new();
    w.u32(p.consts.len() as u32);
    for c in &p.consts {
        w.u8(c.tag());
        match c {
            Const::Unit => {}
            Const::Bool(v) => w.u8(*v as u8),
            Const::I64(v) => w.u64(*v as u64),
            Const::F64(v) => w.u64(v.to_bits()),
            Const::Str(s) => w.str(s),
            Const::EmptyList(index) => w.u32(*index),
        }
    }
    sections.push((section::CONSTANTS, w.finish()));

    let mut w = Writer::new();
    w.u32(p.types.len() as u32);
    for t in &p.types {
        w.u8(t.tag());
        match t {
            TypeDesc::Task(inner) | TypeDesc::List(inner) => w.u32(*inner),
            TypeDesc::Object { name, fields, open } => {
                w.str(name);
                w.u8(*open as u8);
                w.u8(fields.len() as u8);
                for field in fields {
                    w.str(&field.name);
                    w.u32(field.ty);
                    w.u8(field.optional as u8);
                }
            }
            _ => {}
        }
    }
    sections.push((section::TYPES, w.finish()));

    let mut w = Writer::new();
    w.u32(p.funcs.len() as u32);
    for f in &p.funcs {
        w.str(&f.name);
        w.u8(f.params.len() as u8);
        for t in &f.params {
            w.u32(*t);
        }
        w.u8(f.reg_count);
        w.u32(f.ret_type);
        w.u32(f.code_off);
        w.u32(f.code_len);
    }
    sections.push((section::FUNCTIONS, w.finish()));

    let mut w = Writer::new();
    w.u32(p.caps.len() as u32);
    for c in &p.caps {
        w.str(&c.name);
        w.u8(c.kind as u8);
        w.str(&c.constraints);
        w.str(&c.pin);
        // Next to the pin, because they are the two claims a grant makes about
        // the program itself: which one runs, and what comes back.
        w.u8(c.answers as u8);
        w.bool(c.repeatable);
        w.bool(c.delegable);
        w.str(&c.dir);
        w.u32(c.env.len() as u32);
        for (name, value) in &c.env {
            w.str(name);
            w.str(value);
        }
        w.u8(c.args.len() as u8);
        for a in &c.args {
            w.str(a);
        }
        w.u8(c.params.len() as u8);
        for t in &c.params {
            w.u32(*t);
        }
        w.u32(c.ret_type);
    }
    sections.push((section::CAPABILITIES, w.finish()));

    let mut w = Writer::new();
    for inst in &p.code {
        w.u32(inst.0);
    }
    sections.push((section::CODE, w.finish()));

    let mut w = Writer::new();
    w.u32(p.policies.len() as u32);
    for policy in &p.policies {
        w.u32(policy.pc);
        w.u32(policy.attempts);
        w.u32(policy.timeout_ms);
        w.u32(policy.budget);
        w.u32(policy.budget_group);
        w.u32(policy.conversation);
        w.u32(policy.tools);
        w.u32(policy.deadline_ms);
        w.u32(policy.validates);
    }
    sections.push((section::POLICIES, w.finish()));

    let mut w = Writer::new();
    w.u32(p.debug.sources.len() as u32);
    for name in &p.debug.sources {
        w.str(name);
    }
    w.u32(p.debug.lines.len() as u32);
    for (pc, file, line, col) in &p.debug.lines {
        w.u32(*pc);
        w.u32(*file);
        w.u32(*line);
        w.u32(*col);
    }
    sections.push((section::DEBUG, w.finish()));

    // Empty in v0.1. The section exists so that adding signatures later does
    // not change the shape of the file.
    sections.push((section::SIGNATURE, Vec::new()));

    let header_len = 4 + 4 + 4 + 4;
    let table_len = sections.len() * 12;
    let mut body_offset = (header_len + table_len) as u32;

    let mut out = Writer::new();
    out.bytes(&MAGIC);
    out.u16(VERSION_MAJOR);
    out.u16(VERSION_MINOR);
    out.u32(0); // flags
    out.u32(sections.len() as u32);
    for (kind, body) in &sections {
        out.u32(*kind);
        out.u32(body_offset);
        out.u32(body.len() as u32);
        body_offset += body.len() as u32;
    }
    for (_, body) in &sections {
        out.bytes(body);
    }
    out.finish()
}

// ---------------------------------------------------------------- decoding

pub fn decode(bytes: &[u8]) -> Result<Program> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC {
        return Err(DecodeError::new("not a sic bytecode file (bad magic)"));
    }
    let (major, minor) = (r.u16()?, r.u16()?);
    if (major, minor) != (VERSION_MAJOR, VERSION_MINOR) {
        return Err(DecodeError::new(format!(
            "unsupported bytecode version {major}.{minor}, expected {VERSION_MAJOR}.{VERSION_MINOR}"
        )));
    }
    let flags = r.u32()?;
    if flags != 0 {
        return Err(DecodeError::new(format!("unknown flags {flags:#x}")));
    }

    let count = r.u32()?;
    if count > MAX_SECTIONS {
        return Err(DecodeError::new(format!(
            "section count {count} exceeds the limit of {MAX_SECTIONS}"
        )));
    }

    let mut entries: Vec<(u32, u32, u32)> = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let kind = r.u32()?;
        let off = r.u32()?;
        let len = r.u32()?;
        let end = off
            .checked_add(len)
            .ok_or_else(|| DecodeError::new("section range overflows"))?;
        if end as usize > bytes.len() {
            return Err(DecodeError::new(format!(
                "section {kind} runs past the end of the file"
            )));
        }
        if entries.iter().any(|(k, _, _)| *k == kind) {
            return Err(DecodeError::new(format!("section {kind} appears twice")));
        }
        entries.push((kind, off, len));
    }
    no_overlap(&entries)?;

    let mut p = Program::default();
    for (kind, off, len) in entries {
        let body = &bytes[off as usize..(off + len) as usize];
        match kind {
            section::CONSTANTS => p.consts = decode_consts(body)?,
            section::TYPES => p.types = decode_types(body)?,
            section::FUNCTIONS => p.funcs = decode_funcs(body)?,
            section::CAPABILITIES => p.caps = decode_caps(body)?,
            section::CODE => p.code = decode_code(body)?,
            section::POLICIES => p.policies = decode_policies(body)?,
            section::DEBUG => p.debug = decode_debug(body)?,
            section::SIGNATURE => {
                if !body.is_empty() {
                    return Err(DecodeError::new("signatures are not supported in v0.1"));
                }
            }
            // Ignoring an unknown section would be a channel for hidden data to
            // ride along past signature checking.
            other => {
                return Err(DecodeError::new(format!("unknown section kind {other}")));
            }
        }
    }
    Ok(p)
}

/// Refuses a section table whose entries claim the same bytes.
///
/// The third of the three things §9 item 2 of `docs/design/v0.1.md` says
/// decoding establishes, and the one that was not being checked. A check in the
/// specification that is not in the code is worse than one that is in neither:
/// §9 is the list the verifier's contract with the VM is written against, and
/// the VM drops runtime checks because that list says the property holds.
///
/// It matters for the same reason an unknown section kind is refused a few
/// lines below. That comment says ignoring one "would be a channel for hidden
/// data to ride along past signature checking"; aliasing is the same argument
/// from the other side. The `SIGNATURE` section is empty in v0.1 and exists so
/// signatures can be added without changing the shape of the file, and once it
/// is filled in, what was signed is a set of byte ranges - so a file whose
/// sections may alias is one where the bytes a signature covers and the bytes
/// the decoder reads need not be the same set. One comparison now; a format
/// version later.
///
/// Empty sections are skipped: a section of no bytes claims none, and two of
/// them at the same offset are not two names for one byte.
///
/// Gaps are allowed. A file may have bytes no section names, and refusing that
/// is a different question with a different answer - the one there might be
/// "the signature covers the file" rather than "the sections tile it".
fn no_overlap(entries: &[(u32, u32, u32)]) -> Result<()> {
    let mut ranges: Vec<(u32, u32, u32)> = entries
        .iter()
        .filter(|(_, _, len)| *len > 0)
        .map(|(kind, off, len)| (*off, off + len, *kind))
        .collect();
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        let ((_, first_end, first), (second_off, _, second)) = (pair[0], pair[1]);
        if second_off < first_end {
            return Err(DecodeError::new(format!(
                "sections {first} and {second} claim the same bytes"
            )));
        }
    }
    Ok(())
}

fn decode_consts(body: &[u8]) -> Result<Vec<Const>> {
    let mut r = Reader::new(body);
    let n = r.count(1)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = r.u8()?;
        out.push(match tag {
            0 => Const::Unit,
            1 => Const::Bool(r.u8()? != 0),
            2 => Const::I64(r.u64()? as i64),
            3 => Const::F64(f64::from_bits(r.u64()?)),
            4 => Const::Str(r.str()?),
            5 => Const::EmptyList(r.u32()?),
            other => return Err(DecodeError::new(format!("unknown constant tag {other}"))),
        });
    }
    r.expect_end("constants")?;
    Ok(out)
}

fn decode_types(body: &[u8]) -> Result<Vec<TypeDesc>> {
    let mut r = Reader::new(body);
    let n = r.count(1)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let raw = r.u8()?;
        out.push(match raw {
            0 => TypeDesc::Unit,
            1 => TypeDesc::Bool,
            2 => TypeDesc::Int,
            3 => TypeDesc::Float,
            4 => TypeDesc::Str,
            5 => TypeDesc::Task(r.u32()?),
            6 => TypeDesc::List(r.u32()?),
            7 => {
                let name = r.str()?;
                let open = r.u8()? != 0;
                let field_count = r.u8()? as usize;
                let mut fields = Vec::with_capacity(field_count);
                for _ in 0..field_count {
                    let name = r.str()?;
                    let ty = r.u32()?;
                    let optional = r.u8()? != 0;
                    fields.push(Field { name, ty, optional });
                }
                TypeDesc::Object { name, fields, open }
            }
            other => return Err(DecodeError::new(format!("unknown type tag {other}"))),
        });
    }
    r.expect_end("types")?;
    Ok(out)
}

fn decode_funcs(body: &[u8]) -> Result<Vec<FuncDef>> {
    let mut r = Reader::new(body);
    // The smallest possible entry: an empty name, no parameters, and the fixed
    // fields.
    let n = r.count(19)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let param_count = r.u8()? as usize;
        let mut params = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            params.push(r.u32()?);
        }
        out.push(FuncDef {
            name,
            params,
            reg_count: r.u8()?,
            ret_type: r.u32()?,
            code_off: r.u32()?,
            code_len: r.u32()?,
        });
    }
    r.expect_end("functions")?;
    Ok(out)
}

fn decode_caps(body: &[u8]) -> Result<Vec<CapDecl>> {
    let mut r = Reader::new(body);
    let n = r.count(20)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let raw = r.u8()?;
        let kind = CapKind::from_u8(raw)
            .ok_or_else(|| DecodeError::new(format!("unknown capability kind {raw}")))?;
        let constraints = r.str()?;
        let pin = r.str()?;
        let raw = r.u8()?;
        let answers = Answers::from_u8(raw)
            .ok_or_else(|| DecodeError::new(format!("unknown answer shape {raw}")))?;
        let repeatable = r.bool()?;
        let delegable = r.bool()?;
        let dir = r.str()?;
        // Two strings is the smallest a pair can be, which is what stops a
        // claimed count from allocating on a promise.
        let pairs = r.count(2)?;
        let mut env = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            env.push((r.str()?, r.str()?));
        }
        let arg_count = r.u8()? as usize;
        let mut args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            args.push(r.str()?);
        }
        let param_count = r.u8()? as usize;
        let mut params = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            params.push(r.u32()?);
        }
        out.push(CapDecl {
            name,
            kind,
            constraints,
            pin,
            answers,
            repeatable,
            delegable,
            dir,
            env,
            args,
            params,
            ret_type: r.u32()?,
        });
    }
    r.expect_end("capabilities")?;
    Ok(out)
}

fn decode_code(body: &[u8]) -> Result<Vec<Inst>> {
    if body.len() % 4 != 0 {
        return Err(DecodeError::new(
            "the code section is not a whole number of instructions",
        ));
    }
    Ok(body
        .chunks_exact(4)
        .map(|c| Inst(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .collect())
}

fn decode_policies(body: &[u8]) -> Result<Vec<PolicyEntry>> {
    let mut r = Reader::new(body);
    let n = r.count(36)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(PolicyEntry {
            pc: r.u32()?,
            attempts: r.u32()?,
            timeout_ms: r.u32()?,
            budget: r.u32()?,
            budget_group: r.u32()?,
            conversation: r.u32()?,
            tools: r.u32()?,
            deadline_ms: r.u32()?,
            validates: r.u32()?,
        });
    }
    r.expect_end("policies")?;
    Ok(out)
}

fn decode_debug(body: &[u8]) -> Result<DebugInfo> {
    let mut r = Reader::new(body);
    // Each name is at least a length prefix, so one file cannot claim more
    // entries than the body has bytes for.
    let files = r.count(4)?;
    let mut sources = Vec::with_capacity(files);
    for _ in 0..files {
        sources.push(r.str()?);
    }
    let n = r.count(16)?;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        lines.push((r.u32()?, r.u32()?, r.u32()?, r.u32()?));
    }
    r.expect_end("debug")?;
    Ok(DebugInfo { sources, lines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inst::Op;

    fn sample() -> Program {
        Program {
            consts: vec![
                Const::Unit,
                Const::Bool(true),
                Const::I64(-7),
                Const::F64(1.5),
                Const::Str("hi".into()),
            ],
            types: vec![TypeDesc::Unit, TypeDesc::Int],
            funcs: vec![FuncDef {
                name: "main".into(),
                params: vec![1],
                reg_count: 2,
                ret_type: 1,
                code_off: 0,
                code_len: 2,
            }],
            caps: vec![CapDecl {
                name: "process.exec".into(),
                kind: CapKind::Exec,
                constraints: "/usr/bin/true".into(),
                pin: "a".repeat(64),
                answers: Answers::Unsaid,
                repeatable: false,
                delegable: false,
                dir: String::new(),
                env: Vec::new(),
                args: Vec::new(),
                params: vec![4],
                ret_type: 2,
            }],
            code: vec![
                Inst::abx(Op::LoadConst, 0, 2),
                Inst::abc(Op::Return, 0, 0, 0),
            ],
            policies: vec![PolicyEntry {
                pc: 0,
                attempts: 3,
                timeout_ms: 500,
                budget: 8,
                budget_group: 1,
                conversation: 0,
                tools: 0,
                deadline_ms: 0,
                validates: 0,
            }],
            debug: DebugInfo {
                sources: vec!["main.sic".into()],
                lines: vec![(0, 0, 2, 5), (1, 0, 3, 5)],
            },
        }
    }

    #[test]
    fn round_trip() {
        let p = sample();
        let bytes = encode(&p);
        let back = decode(&bytes).expect("decodes");
        assert_eq!(back.consts, p.consts);
        assert_eq!(back.types, p.types);
        assert_eq!(back.code, p.code);
        assert_eq!(back.funcs[0].name, "main");
        assert_eq!(back.funcs[0].reg_count, 2);
        assert_eq!(back.caps[0].name, "process.exec");
        assert_eq!(back.policies, p.policies);
        assert_eq!(back.debug.lines, p.debug.lines);
        assert_eq!(back.debug.position(1), Some((3, 5)));
        assert_eq!(back.debug.position(0), Some((2, 5)));
    }

    /// A module that uses every tag the format defines, so that the round trip
    /// below is a statement about the format rather than about one program.
    ///
    /// It is not meant to be a program that verifies - `EmptyList` names a type
    /// nothing loads, and no function covers the code - because what is under
    /// test here is only that bytes survive the journey.
    fn everything() -> Program {
        Program {
            consts: vec![
                Const::Unit,
                Const::Bool(false),
                Const::I64(i64::MIN),
                // Not NaN: this is compared with `==`, and the point of the
                // comparison is the bits that came back, not float equality.
                Const::F64(f64::MIN_POSITIVE),
                Const::Str(String::new()),
                Const::EmptyList(6),
            ],
            types: vec![
                TypeDesc::Unit,
                TypeDesc::Bool,
                TypeDesc::Int,
                TypeDesc::Float,
                TypeDesc::Str,
                TypeDesc::Task(2),
                TypeDesc::List(7),
                TypeDesc::Object {
                    name: "Answer".into(),
                    fields: vec![Field::new("text", 4), Field::new("score", 2)],
                    open: false,
                },
                // Both settings of the flag, because one of them encodes as a
                // zero byte and a writer that forgot it entirely would still
                // round-trip the other.
                TypeDesc::Object {
                    name: "Line".into(),
                    fields: vec![Field::new("reason", 4)],
                    open: true,
                },
                // Both settings of the per-field flag, in one record and in
                // that order, so a reader that dropped the byte would run the
                // second field's name into the first field's flag.
                TypeDesc::Object {
                    name: "Artifact".into(),
                    fields: vec![
                        Field {
                            name: "executable".into(),
                            ty: 4,
                            optional: true,
                        },
                        Field::new("reason", 4),
                    ],
                    open: false,
                },
            ],
            funcs: vec![
                FuncDef {
                    name: "main".into(),
                    params: Vec::new(),
                    reg_count: 1,
                    ret_type: 0,
                    code_off: 0,
                    code_len: 1,
                },
                FuncDef {
                    name: "with_params".into(),
                    params: vec![2, 4, 7],
                    reg_count: 255,
                    ret_type: 6,
                    code_off: 1,
                    code_len: 1,
                },
            ],
            caps: vec![
                CapDecl {
                    name: "process.exec".into(),
                    kind: CapKind::Exec,
                    constraints: "/usr/bin/git".into(),
                    pin: "b".repeat(64),
                    // Both shapes a grant can name, one per entry, because
                    // the third encodes as a zero byte and a writer that
                    // forgot the field entirely would still round-trip that
                    // one. What the checker allows on a given name is a
                    // different question from what the format carries.
                    answers: Answers::Jsonl,
                    repeatable: false,
                    delegable: false,
                    dir: String::new(),
                    env: Vec::new(),
                    args: vec!["status".into(), "--porcelain".into()],
                    params: vec![4],
                    ret_type: 2,
                },
                CapDecl {
                    name: "fs.read".into(),
                    kind: CapKind::Read,
                    constraints: "./data.txt".into(),
                    pin: String::new(),
                    answers: Answers::Json,
                    repeatable: false,
                    delegable: false,
                    dir: String::new(),
                    env: Vec::new(),
                    args: Vec::new(),
                    params: Vec::new(),
                    ret_type: 4,
                },
            ],
            code: vec![Inst::abc(Op::Return, 0, 0, 0), Inst::abc(Op::Halt, 0, 0, 0)],
            policies: vec![
                PolicyEntry {
                    pc: 0,
                    attempts: 1,
                    timeout_ms: 0,
                    budget: 0,
                    budget_group: 0,
                    conversation: 0,
                    tools: 0,
                    deadline_ms: 0,
                    validates: 0,
                },
                PolicyEntry {
                    pc: 1,
                    attempts: u32::MAX,
                    timeout_ms: 30_000,
                    budget: 4,
                    budget_group: 2,
                    conversation: 9,
                    tools: 200,
                    deadline_ms: 1_800_000,
                    validates: 0,
                },
            ],
            debug: DebugInfo {
                sources: vec!["main.sic".into(), "lib/util.sic".into()],
                lines: vec![(0, 0, 1, 1), (1, 1, 40, 12)],
            },
        }
    }

    /// The whole module, not the fields a test happened to list.
    ///
    /// `round_trip` above checks eight fields, and would not notice an encoder
    /// that dropped a capability's argument prefix - which is a grant the broker
    /// enforces. Comparing the programs makes every field of the format part of
    /// the assertion, including the ones added after this test was written.
    #[test]
    fn every_tag_survives_the_round_trip() {
        let p = everything();
        assert_eq!(decode(&encode(&p)).expect("decodes"), p);
    }

    #[test]
    fn rejects_flags_this_version_does_not_define() {
        // Flags are how the format would say a file needs something this reader
        // does not have, so an unknown one has to stop the read rather than be
        // ignored.
        let mut bytes = encode(&sample());
        bytes[8] = 1;
        assert!(decode(&bytes).unwrap_err().message.contains("flags"));
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let mut bytes = encode(&sample());
        bytes[0] = b'X';
        assert!(decode(&bytes).unwrap_err().message.contains("magic"));

        let mut bytes = encode(&sample());
        bytes[4] = 9; // major version
        assert!(decode(&bytes).unwrap_err().message.contains("version"));
    }

    #[test]
    fn rejects_unknown_section() {
        let mut bytes = encode(&sample());
        // The first section table entry starts at offset 16.
        bytes[16] = 99;
        assert!(
            decode(&bytes)
                .unwrap_err()
                .message
                .contains("unknown section")
        );
    }

    #[test]
    fn rejects_truncated_file() {
        let bytes = encode(&sample());
        for cut in [0, 4, 8, 16, 20, bytes.len() - 1] {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut} should fail");
        }
    }

    #[test]
    fn rejects_code_that_is_not_whole_instructions() {
        let p = Program {
            code: vec![Inst::abc(Op::Halt, 0, 0, 0)],
            ..Program::default()
        };
        let mut bytes = encode(&p);
        // Drop one byte from the end, which is inside the code section.
        bytes.pop();
        // Fix nothing else: the section length now disagrees with the file.
        assert!(decode(&bytes).is_err());
    }
}
