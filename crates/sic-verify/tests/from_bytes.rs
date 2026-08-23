//! From a byte string, through `decode`, into the verifier.
//!
//! `decode` then `verify` is the whole of what stands between a `.sicb` on a
//! disk and the VM, and each half was tested alone. A test that builds a
//! `Program` in Rust asks the verifier about a module this process wrote and
//! can therefore vouch for; a hostile file is exactly the one nobody vouches
//! for. So every case here states its file as bytes and goes through the two
//! calls `sic verify <FILE.sicb>` makes.

use sic_bytecode::file::section;
use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;
use sic_bytecode::{DecodeError, decode, encode};
use sic_verify::{VerifyReport, verify};

/// A module shaped like something `sic compile` emits: one function that calls
/// a granted capability, a policy on the call site, a debug table, and a type
/// section with a compound type in it. It decodes and it verifies.
///
/// Every test below starts here and breaks one thing, so what a test says is
/// the difference between a file that runs and the file it is describing.
fn well_formed() -> Program {
    let mut types = TypeDesc::primitives();
    types.push(TypeDesc::List(2)); // List<Int>, at index 5

    Program {
        consts: vec![
            Const::Str("/usr/bin/true".into()),
            Const::I64(1),
            Const::EmptyList(5),
        ],
        types,
        funcs: vec![FuncDef {
            name: "main".into(),
            params: Vec::new(),
            reg_count: 3,
            ret_type: 2,
            code_off: 0,
            code_len: 3,
        }],
        caps: vec![CapDecl {
            name: "process.exec".into(),
            kind: CapKind::Exec,
            constraints: "/usr/bin/true".into(),
            pin: "a".repeat(64),
            args: Vec::new(),
            params: vec![4],
            ret_type: 2,
        }],
        code: vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::CallCap, 0, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
        policies: vec![PolicyEntry {
            pc: 1,
            attempts: 3,
            timeout_ms: 500,
            budget: 0,
            conversation: 0,
        }],
        debug: DebugInfo {
            sources: vec!["main.sic".into()],
            lines: vec![(0, 0, 2, 5), (2, 0, 3, 5)],
        },
    }
}

/// Breaks a well-formed module, then puts it through the encoder, `decode` and
/// `verify` - the path a file takes off a disk.
///
/// Encoding a module and corrupting it, rather than assembling a file byte by
/// byte, is what keeps these tests alive: the format is at 0.4 and still
/// moving, and a hand-written fixture would need rewriting at every minor
/// version while proving nothing extra. What it costs is that a corruption has
/// to be one the format can carry, which every corruption below is - these are
/// files that decode and must not run, not files that fail to parse.
fn verify_corrupted(corrupt: impl FnOnce(&mut Program)) -> VerifyReport {
    let mut p = well_formed();
    corrupt(&mut p);
    let bytes = encode(&p);
    let program = decode(&bytes).expect("a corruption the format can carry");
    verify(&program)
}

/// Decodes bytes that were encoded and then edited in place.
fn decode_corrupted(edit: impl FnOnce(&mut Vec<u8>)) -> Result<Program, DecodeError> {
    let mut bytes = encode(&well_formed());
    edit(&mut bytes);
    decode(&bytes)
}

/// The offset of one section table entry, which is where a file says where a
/// section is. The table follows the 16-byte header, one 12-byte entry per
/// section, in the order `encode` writes them.
fn section_entry(bytes: &[u8], kind: u32) -> usize {
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    (0..count)
        .map(|i| 16 + i * 12)
        .find(|off| u32::from_le_bytes(bytes[*off..*off + 4].try_into().unwrap()) == kind)
        .expect("the section is in the table")
}

fn set_section_len(bytes: &mut [u8], kind: u32, len: u32) {
    let entry = section_entry(bytes, kind);
    bytes[entry + 8..entry + 12].copy_from_slice(&len.to_le_bytes());
}

/// Asserts that the file was refused, and refused for the stated reason. A test
/// that only checks that verification failed passes for the wrong reason as
/// easily as the right one.
#[track_caller]
fn assert_rejects(report: &VerifyReport, because: &str) {
    assert!(
        !report.ok(),
        "the file verified; it should have been refused for {because:?}"
    );
    assert!(
        report.errors.iter().any(|f| f.message.contains(because)),
        "refused, but not for {because:?}: {:#?}",
        report.errors
    );
}

#[test]
fn a_file_that_survives_the_round_trip_still_verifies() {
    // The floor under every other test here: the harness produces a file that
    // decodes to the module it was built from and passes verification, so a
    // rejection below is the corruption and not the harness.
    let p = well_formed();
    let decoded = decode(&encode(&p)).expect("decodes");
    assert_eq!(decoded, p);
    let report = verify(&decoded);
    assert!(report.ok(), "{:#?}", report.errors);
}

#[test]
fn a_file_that_decodes_and_must_not_run_is_refused_by_the_verifier() {
    // The seam itself. Decoding a code section is reading `u32`s, so an opcode
    // nobody defined is a file the format is happy to produce a `Program` from
    // and the VM must never be handed. Nothing established that jointly until
    // a test read bytes at one end and a diagnostic at the other.
    let report = verify_corrupted(|p| p.code[2] = Inst(u32::MAX));
    assert_rejects(&report, "unknown opcode");
}

// ---- what decode refuses before the verifier is asked ----

#[test]
fn a_section_that_runs_past_the_end_of_the_file_is_refused() {
    // v0.1 section 9 item 2. A section table is a set of claims about a file
    // made by whoever wrote the file, so a length is a request to read, not a
    // fact about what is there.
    let err = decode_corrupted(|bytes| set_section_len(bytes, section::CODE, u32::MAX / 2))
        .expect_err("a section that leaves the file cannot be read");
    assert!(err.message.contains("runs past the end"), "{}", err.message);
}

#[test]
fn bytes_left_over_inside_a_section_are_refused() {
    // A section whose body holds more than its own elements is a place to hide
    // data that travels with the module and that nothing else looks at. Each
    // decoder ends with `expect_end` for that reason, and this is the check.
    let err = decode_corrupted(|bytes| {
        let entry = section_entry(bytes, section::CONSTANTS);
        let len = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap());
        set_section_len(bytes, section::CONSTANTS, len + 1);
    })
    .expect_err("a body longer than its contents is not that body");
    assert!(err.message.contains("left over"), "{}", err.message);
}

#[test]
fn a_non_empty_signature_section_is_refused() {
    // v0.1 has no signatures. The section exists so that adding them later does
    // not change the shape of the file, which means a file claiming to carry
    // one is claiming something this version cannot check - and an unchecked
    // signature is worse than none.
    let err = decode_corrupted(|bytes| {
        // The signature section is last and empty, so its offset is the end of
        // the file: a byte appended to the file is a byte inside it.
        bytes.push(0);
        set_section_len(bytes, section::SIGNATURE, 1);
    })
    .expect_err("a signature this version cannot check");
    assert!(err.message.contains("signatures"), "{}", err.message);
}

#[test]
fn corrupting_a_byte_anywhere_never_gets_past_the_pair() {
    // The property is not that every flipped byte is refused - a byte inside a
    // string constant makes a different program, not a broken one. It is that
    // the two calls always reach a verdict on bytes nobody wrote: a panic in
    // either is `sic verify` taken down by the file it was pointed at, so
    // running this at all is most of the assertion.
    //
    // Deterministic on purpose: every offset, three fixed masks, no seed.
    let bytes = encode(&well_formed());
    for offset in 0..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let mut mutated = bytes.clone();
            mutated[offset] ^= mask;
            let decoded = decode(&mutated);
            // The first twelve bytes name the format itself - magic, version,
            // flags - so nothing in them can be changed and still be this
            // format.
            if offset < 12 {
                assert!(
                    decoded.is_err(),
                    "byte {offset} of the header was changed and the file still decoded"
                );
                continue;
            }
            if let Ok(program) = decoded {
                verify(&program);
            }
        }
    }
}
