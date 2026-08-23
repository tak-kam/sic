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
pub const VERSION_MINOR: u16 = 1;

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
            TypeDesc::Object { name, fields } => {
                w.str(name);
                w.u8(fields.len() as u8);
                for (field_name, field_type) in fields {
                    w.str(field_name);
                    w.u32(*field_type);
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
    }
    sections.push((section::POLICIES, w.finish()));

    let mut w = Writer::new();
    w.str(&p.debug.source_name);
    w.u32(p.debug.lines.len() as u32);
    for (pc, line, col) in &p.debug.lines {
        w.u32(*pc);
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
                let field_count = r.u8()? as usize;
                let mut fields = Vec::with_capacity(field_count);
                for _ in 0..field_count {
                    fields.push((r.str()?, r.u32()?));
                }
                TypeDesc::Object { name, fields }
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
    let n = r.count(18)?;
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
    let n = r.count(18)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let raw = r.u8()?;
        let kind = CapKind::from_u8(raw)
            .ok_or_else(|| DecodeError::new(format!("unknown capability kind {raw}")))?;
        let constraints = r.str()?;
        let pin = r.str()?;
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
    let n = r.count(16)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(PolicyEntry {
            pc: r.u32()?,
            attempts: r.u32()?,
            timeout_ms: r.u32()?,
            budget: r.u32()?,
        });
    }
    r.expect_end("policies")?;
    Ok(out)
}

fn decode_debug(body: &[u8]) -> Result<DebugInfo> {
    let mut r = Reader::new(body);
    let source_name = r.str()?;
    let n = r.count(12)?;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        lines.push((r.u32()?, r.u32()?, r.u32()?));
    }
    r.expect_end("debug")?;
    Ok(DebugInfo { source_name, lines })
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
            }],
            debug: DebugInfo {
                source_name: "main.sic".into(),
                lines: vec![(0, 2, 5), (1, 3, 5)],
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
