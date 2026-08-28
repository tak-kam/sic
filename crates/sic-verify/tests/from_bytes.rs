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
use sic_verify::{MAX_CODE_LEN, MAX_CONSTS, MAX_FUNCS, VerifyReport, verify};

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
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::CallCap, 0, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
        policies: vec![PolicyEntry {
            pc: 1,
            attempts: 3,
            timeout_ms: 500,
            budget: 0,
            budget_group: 0,
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
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

/// The table entry of the last section in the file that has a body.
///
/// The empty ones are skipped because growing one of those is a different test:
/// a signature section that is not empty is refused for being a signature.
fn last_section(bytes: &[u8]) -> usize {
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    (0..count)
        .map(|i| 16 + i * 12)
        .filter(|off| u32::from_le_bytes(bytes[*off + 8..*off + 12].try_into().unwrap()) > 0)
        .max_by_key(|off| u32::from_le_bytes(bytes[*off + 4..*off + 8].try_into().unwrap()))
        .expect("the table has a section with a body")
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

/// The shape a grant claims its program answers in is one byte of the
/// manifest, and a byte the format does not define is not a shape.
///
/// A decoder that took an unknown value for `Unsaid` would turn a file it does
/// not understand into a manifest that claims less than the one that was
/// written - which is the direction that lets a check be skipped by writing a 3.
#[test]
fn a_shape_the_format_does_not_define_is_refused() {
    let refused = decode_corrupted(|bytes| {
        // The byte after the pin, in the one capability entry `well_formed`
        // has. Found rather than counted, so this does not have to be edited
        // every time a field moves.
        let entry = section_entry(bytes, section::CAPABILITIES);
        let off = u32::from_le_bytes(bytes[entry + 4..entry + 8].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap()) as usize;
        let pin = "a".repeat(64);
        let at = bytes[off..off + len]
            .windows(pin.len())
            .position(|w| w == pin.as_bytes())
            .expect("the pin is in the section");
        bytes[off + at + pin.len()] = 3;
    })
    .expect_err("a byte that is not a shape must not decode");
    assert!(
        refused.to_string().contains("unknown answer shape 3"),
        "{refused}"
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
    // Grown into a byte added at the end of the file rather than into its
    // neighbour: a section that reaches into the next one is refused earlier,
    // for aliasing, and this test is about what a decoder does with a body it
    // can read to the end of and still have bytes left.
    let err = decode_corrupted(|bytes| {
        let last = last_section(bytes);
        let kind = u32::from_le_bytes(bytes[last..last + 4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[last + 8..last + 12].try_into().unwrap());
        bytes.push(0);
        set_section_len(bytes, kind, len + 1);
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

// ---- the module-level pass: a file that decodes and lies ----
//
// Everything below is a well-formed module with one thing changed, so what each
// test says is exactly the lie the verifier has to catch. These are the checks
// that exist because a file might be hostile rather than merely wrong, which is
// why they are stated as files rather than as `Program` values.

#[test]
fn more_functions_than_the_limit_are_refused() {
    // Cheap: every added function points at the three instructions that are
    // already there, so the file grows by one table entry each rather than by a
    // body. Nothing here allocates the limit in anything but names.
    //
    // The second assertion answers a question the limit left open. `check_module`
    // does not return after reporting it, so all 257 functions are verified
    // anyway; they pass, and the report is the one error. A limit that fires and
    // then lets the pass it was protecting run to completion is worth knowing
    // about either way.
    let report = verify_corrupted(|p| {
        let template = p.funcs[0].clone();
        for i in 0..MAX_FUNCS {
            p.funcs.push(FuncDef {
                name: format!("f{i}"),
                ..template.clone()
            });
        }
    });
    assert_rejects(
        &report,
        &format!(
            "{} functions exceed the limit of {MAX_FUNCS}",
            MAX_FUNCS + 1
        ),
    );
    assert_eq!(report.errors.len(), 1, "{:#?}", report.errors);
}

#[test]
fn more_constants_than_the_limit_are_refused() {
    // `Const::Unit` is one byte on the wire and no bytes of payload, so the
    // whole constant section here is 64 KiB of tags.
    let report = verify_corrupted(|p| p.consts.resize(MAX_CONSTS + 1, Const::Unit));
    assert_rejects(
        &report,
        &format!(
            "{} constants exceed the limit of {MAX_CONSTS}",
            MAX_CONSTS + 1
        ),
    );
    assert_eq!(report.errors.len(), 1, "{:#?}", report.errors);
}

#[test]
fn more_code_than_the_limit_is_refused() {
    // The one test here that pays for itself in memory: a megabyte of
    // instructions is four megabytes of file, encoded and decoded once. It is
    // worth paying, because the limit is on the size of the code section and
    // there is no way to exceed it without a code section that large.
    //
    // What is avoided is the expensive half. The function still covers three
    // instructions, so the data-flow pass - the part whose cost is what the
    // limit exists to bound - never walks the padding.
    let report = verify_corrupted(|p| {
        p.code
            .resize(MAX_CODE_LEN + 1, Inst::abc(Op::Halt, 0, 0, 0))
    });
    assert_rejects(
        &report,
        &format!(
            "{} instructions exceed the limit of {MAX_CODE_LEN}",
            MAX_CODE_LEN + 1
        ),
    );
    assert_eq!(report.errors.len(), 1, "{:#?}", report.errors);
}

#[test]
fn a_type_section_that_does_not_begin_with_the_primitives_is_refused() {
    // The load-bearing one. Everything after this check assumes that type index
    // 2 is `Int` and 4 is `String` - the verifier's own `INT` and `STR` are
    // those numbers - so a file that renames an entry of the prefix makes every
    // later type comparison a comparison of something else.
    let report = verify_corrupted(|p| p.types[2] = TypeDesc::Str);
    assert_rejects(&report, "must begin with the primitive types in tag order");
}

#[test]
fn a_type_that_names_a_type_outside_the_section_is_refused() {
    // v0.1 section 9 item 5. `List<t99>` in a section with six entries: the
    // verifier answers "what does indexing this produce" out of the section, so
    // an index it cannot follow has to stop here.
    let report = verify_corrupted(|p| p.types[5] = TypeDesc::List(99));
    assert_rejects(&report, "type 5 refers to type 99, which is out of range");
}

#[test]
fn a_capability_that_names_a_type_outside_the_section_is_refused() {
    // The manifest is the contract with the broker, and a parameter type is
    // half of what a call site is checked against.
    let report = verify_corrupted(|p| p.caps[0].params[0] = 99);
    assert_rejects(&report, "capability c0 names type index 99");
}

#[test]
fn an_empty_list_constant_that_names_a_type_outside_the_section_is_refused() {
    // An empty list carries the type it is empty of, because it has no elements
    // to carry it. That makes a constant a place a type index can hide.
    let report = verify_corrupted(|p| {
        p.consts[2] = Const::EmptyList(99);
        p.code[0] = Inst::abx(Op::LoadConst, 1, 2);
    });
    assert_rejects(&report, "constant k2 names type 99, which is out of range");
}

#[test]
fn two_functions_with_one_name_are_refused() {
    // A name is how `sic run` finds `main` and how a checkpoint names the frame
    // it was taken in, so two functions answering to one name is a file where
    // those questions have no single answer.
    let report = verify_corrupted(|p| {
        let clone = p.funcs[0].clone();
        p.funcs.push(clone);
    });
    assert_rejects(&report, "function name is also used by function 0");
}

#[test]
fn a_debug_entry_that_names_a_pc_past_the_code_is_refused() {
    // The debug table is what a failure and a trace are rendered through, so an
    // entry pointing outside the code is a lie told at the moment something has
    // already gone wrong.
    let report = verify_corrupted(|p| p.debug.lines.push((9999, 0, 1, 1)));
    assert_rejects(&report, "the debug table names pc 9999");
}

#[test]
fn a_policy_on_something_that_is_not_a_capability_call_is_refused() {
    // A policy is retries, a deadline and a budget for one call site. Attached
    // to an instruction that performs no effect it is a grant with nothing to
    // grant, and the pc is the only thing tying the two together.
    let report = verify_corrupted(|p| p.policies[0].pc = 2);
    assert_rejects(
        &report,
        "a policy names instruction 2, which is not a capability call",
    );
}

#[test]
fn a_policy_that_allows_no_attempts_is_refused() {
    // `attempts` is total attempts, not extra ones, so zero is not "do not
    // retry" - it is a call site that can never run, written where a reader
    // would see a retry policy.
    let report = verify_corrupted(|p| p.policies[0].attempts = 0);
    assert_rejects(&report, "the policy at 1 allows zero attempts");
}

#[test]
fn a_function_whose_code_runs_past_the_code_section_is_refused() {
    // v0.1 section 9 item 6, and the check the whole per-function pass is
    // indexed behind: everything after it reads `code[code_off + i]` directly.
    let report = verify_corrupted(|p| p.funcs[0].code_off = 9999);
    assert_rejects(&report, "the function's code runs past the code section");
}

#[test]
fn a_function_with_no_instructions_is_refused() {
    // An empty function has no last instruction, so it cannot end in RETURN,
    // and control would leave it by falling off the end into whatever follows
    // in the code section.
    let report = verify_corrupted(|p| p.funcs[0].code_len = 0);
    assert_rejects(&report, "the function has no instructions");
}

#[test]
fn parameters_that_do_not_fit_in_the_registers_are_refused() {
    // v0.1 section 9 item 7. The entry frame is `reg_count` registers with the
    // parameters written into the first of them, so a function claiming more
    // parameters than registers describes a frame that cannot be built.
    let report = verify_corrupted(|p| p.funcs[0].params = vec![2, 2, 2, 2]);
    assert_rejects(&report, "4 parameters do not fit in 3 registers");
}

#[test]
fn a_function_whose_return_type_is_outside_the_section_is_refused() {
    // v0.1 section 9 item 5 again, on the signature rather than inside the
    // body: a caller is checked against these indices.
    let report = verify_corrupted(|p| p.funcs[0].ret_type = 99);
    assert_rejects(&report, "type index 99 is out of range");
}

#[test]
fn a_call_to_a_function_that_does_not_exist_is_refused() {
    // The last of section 9 item 5's four index kinds. The callee decides how
    // many argument registers a call site needs, so an index out of range is
    // also a frame nobody can size.
    let report = verify_corrupted(|p| {
        p.code[1] = Inst::abc(Op::Call, 0, 7, 1);
        // The policy named that instruction while it was a capability call, and
        // a policy on anything else is a different error in a different test.
        p.policies.clear();
    });
    assert_rejects(&report, "function index f7 is out of range");
}

// ---- the register window of an instruction that passes several ----
//
// Five opcodes pass a contiguous run of registers, and four of them - `CALL`,
// `CALL_CAP`, `SPAWN`, `MAKE_OBJECT` - read its first register out of `c`,
// while `MAKE_LIST` reads it out of `b` because its `c` is how many elements
// there are. The three tests below state that difference as files, so that a
// reading of the check which takes the fifth for a typo fails here rather than
// in a run: a `MAKE_LIST` window measured from `c` would let the first file
// through, and would refuse the second, which is a program that works.

#[test]
fn capability_arguments_outside_the_frame_are_refused() {
    // v0.1 section 9 item 7 at a call site. `CALL_CAP` takes one argument here
    // and reads it from r3 of a three-register frame, so the argument the
    // broker is handed is a register the frame does not have.
    let report = verify_corrupted(|p| p.code[1] = Inst::abc(Op::CallCap, 0, 0, 3));
    assert_rejects(&report, "arguments r3..r4 do not fit in reg_count 3");
}

#[test]
fn make_list_elements_outside_the_frame_are_refused() {
    // The same item for `MAKE_LIST`, arranged so that only the right reading
    // catches it: one element based at r3 in a three-register frame. Measured
    // from `b` the window is r3..r4 and leaves the frame; measured from `c` it
    // would be r1..r2 and fit.
    let report = verify_corrupted(|p| {
        p.code = vec![
            Inst::abx(Op::LoadConst, 0, 1),
            Inst::abc(Op::MakeList, 1, 3, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ];
        // The policy named instruction 1 while it was the capability call.
        p.policies.clear();
    });
    assert_rejects(&report, "elements r3..r4 do not fit in reg_count 3");
}

#[test]
fn a_make_list_whose_elements_start_at_b_still_verifies() {
    // The other direction, and the one a wrong reading breaks silently: two
    // elements based at r0 of a three-register frame. The window is r0..r2 and
    // fits; measured from `c` it would be r2..r4 and this program - which the
    // compiler emits - would be refused as malformed.
    let mut p = well_formed();
    p.code = vec![
        Inst::abx(Op::LoadConst, 0, 1),
        Inst::abx(Op::LoadConst, 1, 1),
        Inst::abc(Op::MakeList, 2, 0, 2),
        Inst::abc(Op::Return, 0, 0, 0),
    ];
    p.funcs[0].code_len = 4;
    p.policies.clear();

    let program = decode(&encode(&p)).expect("decodes");
    let report = verify(&program);
    assert!(report.ok(), "{:#?}", report.errors);
}

/// Two sections claiming the same bytes are refused.
///
/// The third of the three things v0.1 §9 item 2 says decoding establishes, and
/// the one that was not being checked. It matters for the same reason an
/// unknown section kind is refused: what a signature covers is a set of byte
/// ranges, and a file whose sections may alias is one where the bytes signed
/// and the bytes read need not be the same set.
#[test]
fn sections_that_claim_the_same_bytes_are_refused() {
    // The plainest case there is: point one entry at another's offset, change
    // nothing else. Both are empty in a well-formed file, so give them a body
    // to claim first.
    let refused = decode_corrupted(|bytes| {
        let target = section_entry(bytes, section::FUNCTIONS);
        let off = u32::from_le_bytes(bytes[target + 4..target + 8].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[target + 8..target + 12].try_into().unwrap());
        let entry = section_entry(bytes, section::CODE);
        bytes[entry + 4..entry + 8].copy_from_slice(&off.to_le_bytes());
        bytes[entry + 8..entry + 12].copy_from_slice(&(len & !3).to_le_bytes());
    })
    .expect_err("a file whose sections alias must not decode");
    assert!(
        refused.to_string().contains("claim the same bytes"),
        "{refused}"
    );

    // Identical ranges, which is the case that used to go all the way through
    // to a program that verified.
    let refused = decode_corrupted(|bytes| {
        let from = section_entry(bytes, section::TYPES);
        let off = u32::from_le_bytes(bytes[from + 4..from + 8].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[from + 8..from + 12].try_into().unwrap());
        let entry = section_entry(bytes, section::CAPABILITIES);
        bytes[entry + 4..entry + 8].copy_from_slice(&off.to_le_bytes());
        bytes[entry + 8..entry + 12].copy_from_slice(&len.to_le_bytes());
    })
    .expect_err("identical ranges are the plainest overlap there is");
    assert!(
        refused.to_string().contains("claim the same bytes"),
        "{refused}"
    );
}

/// A section of no bytes claims none, so two of them at one offset are not two
/// names for one byte - and a file is still allowed bytes no section names.
#[test]
fn empty_sections_and_gaps_are_still_allowed() {
    let program = decode_corrupted(|bytes| {
        // `SIGNATURE` is empty in v0.1. Point it inside another section: it
        // claims no bytes, so it aliases nothing.
        let types = section_entry(bytes, section::TYPES);
        let off = u32::from_le_bytes(bytes[types + 4..types + 8].try_into().unwrap());
        let entry = section_entry(bytes, section::SIGNATURE);
        bytes[entry + 4..entry + 8].copy_from_slice(&off.to_le_bytes());
    })
    .expect("an empty section claims no bytes");
    assert!(!program.types.is_empty());
}
