//! Checkpoints: the state of a suspended run, written out and read back.
//!
//! A run stops at a capability that cannot answer yet, and everything needed to
//! continue is already in the VM - program counter, registers, call stack,
//! arena, the pending call, and where the journal had got to. Durable execution
//! is therefore writing out state that exists, not a second mechanism beside a
//! synchronous call. That is why the VM suspends instead of calling the broker.
//!
//! **A checkpoint holds values, and the journal does not.** They are different
//! things: the journal is an account of a run that leaves the process, so it
//! records digests; a checkpoint is the run itself, so it has to hold the
//! registers and the arena as they are. Protecting a checkpoint at rest is a
//! separate problem, and not one that recording less would solve.
//!
//! A checkpoint is read back with the same suspicion as bytecode. It comes from
//! a file, so it can be truncated, corrupt or hostile, and a VM restored from
//! one must not start out with its invariants already broken.

use sic_core::bin::{Reader, Writer};
use sic_core::{Digest, Sha256};

use crate::value::{Handle, Value};

pub const MAGIC: [u8; 4] = *b"SICC";
pub const VERSION_MAJOR: u16 = 0;
pub const VERSION_MINOR: u16 = 1;

pub type CheckpointError = sic_core::BinError;

type Result<T> = std::result::Result<T, CheckpointError>;

/// The state of a suspended run.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    /// The digest of the bytecode this run belongs to. Resuming against
    /// anything else would continue one program inside another.
    pub program_digest: Digest,
    pub run: u128,
    /// Where the journal had got to, so that a resumed run continues one
    /// sequence rather than starting a second.
    pub seq: u64,
    pub next_span: u64,
    pub fuel: u64,
    pub pending: Pending,
    pub frames: Vec<Frame>,
    pub regs: Vec<Value>,
    /// Handles of the string constants, which are allocated before the run and
    /// so cannot be rebuilt without duplicating them in the arena.
    pub str_consts: Vec<Option<u32>>,
    pub strings: Vec<String>,
}

/// The capability call the run is waiting on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// Absolute register the answer goes into.
    pub reg: u32,
    pub cap: String,
    pub span: u64,
    pub parent: Option<u64>,
    /// What is being waited for, for whoever has to answer it. This is a value,
    /// which is why it belongs in a checkpoint and not in the journal.
    pub question: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub func: u32,
    pub pc: u32,
    pub reg_base: u32,
    pub ret_reg: u32,
    pub span: u64,
    pub parent: Option<u64>,
}

impl Checkpoint {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(&MAGIC);
        w.u16(VERSION_MAJOR);
        w.u16(VERSION_MINOR);
        w.bytes(self.program_digest.bytes());
        w.u128(self.run);
        w.u64(self.seq);
        w.u64(self.next_span);
        w.u64(self.fuel);

        w.u32(self.pending.reg);
        w.str(&self.pending.cap);
        w.u64(self.pending.span);
        write_option_u64(&mut w, self.pending.parent);
        w.str(&self.pending.question);

        w.u32(self.frames.len() as u32);
        for frame in &self.frames {
            w.u32(frame.func);
            w.u32(frame.pc);
            w.u32(frame.reg_base);
            w.u32(frame.ret_reg);
            w.u64(frame.span);
            write_option_u64(&mut w, frame.parent);
        }

        w.u32(self.regs.len() as u32);
        for value in &self.regs {
            write_value(&mut w, value);
        }

        w.u32(self.str_consts.len() as u32);
        for handle in &self.str_consts {
            match handle {
                Some(h) => {
                    w.bool(true);
                    w.u32(*h);
                }
                None => w.bool(false),
            }
        }

        w.u32(self.strings.len() as u32);
        for s in &self.strings {
            w.str(s);
        }
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Checkpoint> {
        let mut r = Reader::new(bytes);
        if r.take(4)? != MAGIC {
            return Err(CheckpointError::new("not a sic checkpoint (bad magic)"));
        }
        let (major, minor) = (r.u16()?, r.u16()?);
        if (major, minor) != (VERSION_MAJOR, VERSION_MINOR) {
            return Err(CheckpointError::new(format!(
                "unsupported checkpoint version {major}.{minor}, expected {VERSION_MAJOR}.{VERSION_MINOR}"
            )));
        }

        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(r.take(32)?);
        let program_digest = Digest::from_bytes(digest_bytes);

        let run = r.u128()?;
        let seq = r.u64()?;
        let next_span = r.u64()?;
        let fuel = r.u64()?;

        let pending = Pending {
            reg: r.u32()?,
            cap: r.str()?,
            span: r.u64()?,
            parent: read_option_u64(&mut r)?,
            question: r.str()?,
        };

        let frame_count = r.count(28)?;
        let mut frames = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            frames.push(Frame {
                func: r.u32()?,
                pc: r.u32()?,
                reg_base: r.u32()?,
                ret_reg: r.u32()?,
                span: r.u64()?,
                parent: read_option_u64(&mut r)?,
            });
        }

        let reg_count = r.count(1)?;
        let mut regs = Vec::with_capacity(reg_count);
        for _ in 0..reg_count {
            regs.push(read_value(&mut r)?);
        }

        let const_count = r.count(1)?;
        let mut str_consts = Vec::with_capacity(const_count);
        for _ in 0..const_count {
            str_consts.push(if r.bool()? { Some(r.u32()?) } else { None });
        }

        let string_count = r.count(4)?;
        let mut strings = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            strings.push(r.str()?);
        }

        r.expect_end("the checkpoint")?;

        let checkpoint = Checkpoint {
            program_digest,
            run,
            seq,
            next_span,
            fuel,
            pending,
            frames,
            regs,
            str_consts,
            strings,
        };
        checkpoint.check_consistency()?;
        Ok(checkpoint)
    }

    /// Checks what the VM would otherwise have to assume.
    ///
    /// This is the same contract the bytecode verifier has: a restored VM skips
    /// checks only because they happened here. Everything a hostile file could
    /// point somewhere wrong is checked against the state it points into.
    fn check_consistency(&self) -> Result<()> {
        if self.frames.is_empty() {
            return Err(CheckpointError::new(
                "a suspended run has at least one frame",
            ));
        }
        let regs = self.regs.len() as u32;
        if self.pending.reg >= regs {
            return Err(CheckpointError::new(
                "the pending call writes to a register that does not exist",
            ));
        }
        for (i, frame) in self.frames.iter().enumerate() {
            if frame.reg_base > regs || frame.ret_reg >= regs {
                return Err(CheckpointError::new(format!(
                    "frame {i} refers to registers outside the saved stack"
                )));
            }
        }
        // Frames sit one above another in a single register stack, and the
        // arithmetic that finds a register assumes that ordering.
        for pair in self.frames.windows(2) {
            if pair[1].reg_base < pair[0].reg_base {
                return Err(CheckpointError::new(
                    "frames are not in order of their register windows",
                ));
            }
        }
        let strings = self.strings.len() as u32;
        for value in &self.regs {
            if let Value::Str(h) | Value::List(h) | Value::Object(h) = value {
                if h.0 >= strings {
                    return Err(CheckpointError::new(
                        "a saved value points outside the saved arena",
                    ));
                }
            }
        }
        for handle in self.str_consts.iter().flatten() {
            if *handle >= strings {
                return Err(CheckpointError::new(
                    "a string constant points outside the saved arena",
                ));
            }
        }
        Ok(())
    }

    /// The digest of the encoded checkpoint, which names it in the journal.
    pub fn digest(bytes: &[u8]) -> Digest {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finish()
    }
}

fn write_option_u64(w: &mut Writer, value: Option<u64>) {
    match value {
        Some(v) => {
            w.bool(true);
            w.u64(v);
        }
        None => w.bool(false),
    }
}

fn read_option_u64(r: &mut Reader<'_>) -> Result<Option<u64>> {
    Ok(if r.bool()? { Some(r.u64()?) } else { None })
}

fn write_value(w: &mut Writer, value: &Value) {
    match value {
        Value::Unit => w.u8(0),
        Value::Bool(v) => {
            w.u8(1);
            w.bool(*v);
        }
        Value::I64(v) => {
            w.u8(2);
            w.i64(*v);
        }
        Value::F64(v) => {
            w.u8(3);
            w.f64(*v);
        }
        Value::Str(h) => {
            w.u8(4);
            w.u32(h.0);
        }
        Value::List(h) => {
            w.u8(5);
            w.u32(h.0);
        }
        Value::Object(h) => {
            w.u8(6);
            w.u32(h.0);
        }
    }
}

fn read_value(r: &mut Reader<'_>) -> Result<Value> {
    Ok(match r.u8()? {
        0 => Value::Unit,
        1 => Value::Bool(r.bool()?),
        2 => Value::I64(r.i64()?),
        3 => Value::F64(r.f64()?),
        4 => Value::Str(Handle(r.u32()?)),
        5 => Value::List(Handle(r.u32()?)),
        6 => Value::Object(Handle(r.u32()?)),
        other => {
            return Err(CheckpointError::new(format!(
                "unknown value tag {other} in a checkpoint"
            )));
        }
    })
}
