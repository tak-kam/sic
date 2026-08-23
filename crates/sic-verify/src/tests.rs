use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;

/// The type section holds the primitives in tag order, so a primitive is its
/// own index.
fn index_of(desc: TypeDesc) -> u32 {
    desc.primitive_index().expect("a primitive type")
}

use super::verify;

/// Builds a single-function program. The type section lists the tags in tag
/// order, so a `TypeDesc` is its own index.
fn program(
    params: &[TypeDesc],
    ret: TypeDesc,
    reg_count: u8,
    consts: Vec<Const>,
    code: Vec<Inst>,
) -> Program {
    Program {
        consts,
        types: TypeDesc::primitives(),
        funcs: vec![FuncDef {
            name: "f".into(),
            params: params.iter().map(|t| index_of(t.clone())).collect(),
            reg_count,
            ret_type: index_of(ret),
            code_off: 0,
            code_len: code.len() as u32,
        }],
        caps: Vec::new(),
        code,
        policies: Vec::new(),
        debug: DebugInfo::default(),
    }
}

fn errors(p: &Program) -> Vec<String> {
    verify(p).errors.iter().map(|f| f.message.clone()).collect()
}

fn assert_ok(p: &Program) {
    let report = verify(p);
    assert!(report.ok(), "unexpected errors: {:#?}", report.errors);
}

#[test]
fn accepts_a_valid_function() {
    // f(a: Int) -> Int { return a + 1 }
    let p = program(
        &[TypeDesc::Int],
        TypeDesc::Int,
        3,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::AddI64, 2, 0, 1),
            Inst::abc(Op::Return, 2, 0, 0),
        ],
    );
    assert_ok(&p);
}

#[test]
fn rejects_reading_an_uninitialized_register() {
    let p = program(
        &[],
        TypeDesc::Int,
        2,
        vec![],
        vec![Inst::abc(Op::Return, 1, 0, 0)],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("before it is written")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_the_wrong_operand_type() {
    // ADD_I64 on a Bool.
    let p = program(
        &[],
        TypeDesc::Int,
        2,
        vec![Const::Bool(true)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::AddI64, 1, 0, 0),
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("holds Bool where Int is required")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_a_return_of_the_wrong_type() {
    let p = program(
        &[],
        TypeDesc::Int,
        1,
        vec![Const::Bool(false)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("holds Bool where Int is required")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_out_of_range_indices() {
    let p = program(
        &[],
        TypeDesc::Int,
        1,
        vec![],
        vec![
            Inst::abx(Op::LoadConst, 0, 7),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("constant index k7")),
        "{:?}",
        errors(&p)
    );

    let p = program(
        &[],
        TypeDesc::Unit,
        1,
        vec![Const::Unit],
        vec![
            Inst::abx(Op::LoadConst, 5, 0),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("beyond reg_count")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_a_jump_out_of_the_function() {
    let p = program(
        &[],
        TypeDesc::Unit,
        1,
        vec![Const::Unit],
        vec![Inst::abx(Op::LoadConst, 0, 0), Inst::asbx(Op::Jump, 0, 40)],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("outside the function")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_falling_off_the_end() {
    let p = program(
        &[],
        TypeDesc::Unit,
        1,
        vec![Const::Unit],
        vec![Inst::abx(Op::LoadConst, 0, 0)],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("fall out of the function")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn rejects_an_unknown_opcode() {
    let p = program(
        &[],
        TypeDesc::Unit,
        1,
        vec![Const::Unit],
        vec![Inst(0xFF), Inst::abc(Op::Halt, 0, 0, 0)],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("unknown opcode")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_register_written_on_only_one_path_stays_uninitialized() {
    // if c { r1 = 1 }  return r1
    let p = program(
        &[TypeDesc::Bool],
        TypeDesc::Int,
        2,
        vec![Const::I64(1)],
        vec![
            Inst::asbx(Op::JumpIfNot, 0, 1), // skip the load
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("before it is written")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_register_with_two_types_becomes_ambiguous() {
    // if c { r1 = 1 } else { r1 = true }  then use r1
    let p = program(
        &[TypeDesc::Bool],
        TypeDesc::Int,
        2,
        vec![Const::I64(1), Const::Bool(true)],
        vec![
            Inst::asbx(Op::JumpIfNot, 0, 2),
            Inst::abx(Op::LoadConst, 1, 0), // r1 = 1
            Inst::asbx(Op::Jump, 0, 1),
            Inst::abx(Op::LoadConst, 1, 1), // r1 = true
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("different types depending on the path")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_register_written_on_both_paths_is_fine() {
    let p = program(
        &[TypeDesc::Bool],
        TypeDesc::Int,
        2,
        vec![Const::I64(1), Const::I64(2)],
        vec![
            Inst::asbx(Op::JumpIfNot, 0, 2),
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::asbx(Op::Jump, 0, 1),
            Inst::abx(Op::LoadConst, 1, 1),
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert_ok(&p);
}

#[test]
fn unreachable_code_is_a_warning_not_an_error() {
    let p = program(
        &[],
        TypeDesc::Int,
        1,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::Return, 0, 0, 0),
            Inst::abc(Op::Return, 0, 0, 0), // never reached
        ],
    );
    let report = verify(&p);
    assert!(report.ok(), "{:#?}", report.errors);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].message.contains("unreachable"));
}

#[test]
fn call_arguments_are_checked() {
    let mut p = program(
        &[TypeDesc::Int],
        TypeDesc::Int,
        1,
        vec![],
        vec![Inst::abc(Op::Return, 0, 0, 0)],
    );
    // A second function that calls the first with a Bool.
    p.funcs.push(FuncDef {
        name: "caller".into(),
        params: Vec::new(),
        reg_count: 2,
        ret_type: 2,
        code_off: 1,
        code_len: 3,
    });
    p.consts.push(Const::Bool(true));
    p.code.push(Inst::abx(Op::LoadConst, 1, 0));
    p.code.push(Inst::abc(Op::Call, 0, 0, 1));
    p.code.push(Inst::abc(Op::Return, 0, 0, 0));

    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("holds Bool where Int is required")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn call_arguments_must_fit_in_the_frame() {
    let mut p = program(
        &[TypeDesc::Int],
        TypeDesc::Int,
        1,
        vec![],
        vec![Inst::abc(Op::Return, 0, 0, 0)],
    );
    p.funcs.push(FuncDef {
        name: "caller".into(),
        params: Vec::new(),
        reg_count: 1,
        ret_type: 2,
        code_off: 1,
        code_len: 2,
    });
    p.code.push(Inst::abc(Op::Call, 0, 0, 1)); // argument at r1, but reg_count is 1
    p.code.push(Inst::abc(Op::Return, 0, 0, 0));

    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("do not fit in reg_count")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn comparing_two_different_types_is_rejected() {
    let p = program(
        &[],
        TypeDesc::Bool,
        3,
        vec![Const::I64(1), Const::Bool(true)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abx(Op::LoadConst, 1, 1),
            Inst::abc(Op::Eq, 2, 0, 1),
            Inst::abc(Op::Return, 2, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("cannot compare")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_loop_converges() {
    // A backward jump: the fixed point must terminate rather than spin.
    let p = program(
        &[TypeDesc::Bool],
        TypeDesc::Int,
        2,
        vec![Const::I64(0)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::asbx(Op::JumpIf, 0, -2), // back to the load
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert_ok(&p);
}

// ---- capabilities ----

/// A program with one capability: `process.exec(String) -> Int`.
fn with_exec_capability(reg_count: u8, consts: Vec<Const>, code: Vec<Inst>) -> Program {
    let mut p = program(&[], TypeDesc::Int, reg_count, consts, code);
    p.caps.push(CapDecl {
        name: "process.exec".into(),
        kind: CapKind::Exec,
        constraints: "/usr/bin/true".into(),
        pin: String::new(),
        args: Vec::new(),
        params: vec![4],
        ret_type: 2,
    });
    p
}

#[test]
fn a_capability_call_is_checked_against_the_manifest() {
    let p = with_exec_capability(
        2,
        vec![Const::Str("/usr/bin/true".into())],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::CallCap, 0, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert_ok(&p);
}

#[test]
fn a_capability_call_must_name_a_declared_capability() {
    let p = with_exec_capability(
        2,
        vec![Const::Str("x".into())],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::CallCap, 0, 7, 1), // no such entry
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("not in the manifest")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn capability_arguments_are_type_checked() {
    let p = with_exec_capability(
        2,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0), // an Int where a String is required
            Inst::abc(Op::CallCap, 0, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("holds Int where String is required")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_granted_but_uncalled_capability_is_a_warning() {
    // Authority the module does not use should never pass unnoticed.
    let p = with_exec_capability(
        1,
        vec![Const::I64(0)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    let report = verify(&p);
    assert!(report.ok(), "{:#?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.message.contains("never called")),
        "{:#?}",
        report.warnings
    );
}

// ---- records and lists ----

#[test]
fn the_verifier_knows_what_a_field_produces() {
    let mut p = program(
        &[],
        TypeDesc::Int,
        3,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::MakeObject, 2, 5, 1),
            Inst::abc(Op::GetField, 0, 2, 0),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    p.types.push(TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![("value".into(), index_of(TypeDesc::Int))],
    });
    assert_ok(&p);
}

#[test]
fn a_field_of_the_wrong_type_is_rejected() {
    let mut p = program(
        &[],
        TypeDesc::Int,
        3,
        vec![Const::Bool(true)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::MakeObject, 2, 5, 1),
            Inst::abc(Op::GetField, 0, 2, 0),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    p.types.push(TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![("value".into(), index_of(TypeDesc::Int))],
    });
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("holds Bool where Int is required")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_field_that_does_not_exist_is_rejected() {
    let mut p = program(
        &[],
        TypeDesc::Int,
        3,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::MakeObject, 2, 5, 1),
            Inst::abc(Op::GetField, 0, 2, 7),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    p.types.push(TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![("value".into(), index_of(TypeDesc::Int))],
    });
    assert!(
        errors(&p).iter().any(|m| m.contains("has no field 7")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn indexing_a_non_list_is_rejected() {
    let p = program(
        &[],
        TypeDesc::Int,
        3,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::GetIndex, 2, 0, 1),
            Inst::abc(Op::Return, 2, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("cannot be indexed")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn a_list_needs_its_type_in_the_section() {
    // Without it the verifier could not say what indexing produces.
    let p = program(
        &[],
        TypeDesc::Int,
        3,
        vec![Const::I64(1)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::MakeList, 1, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
    );
    assert!(
        errors(&p).iter().any(|m| m.contains("no `List<Int>`")),
        "{:?}",
        errors(&p)
    );
}

#[test]
fn len_needs_a_list_or_a_string() {
    let p = program(
        &[],
        TypeDesc::Int,
        2,
        vec![Const::Bool(true)],
        vec![
            Inst::abx(Op::LoadConst, 0, 0),
            Inst::abc(Op::Len, 1, 0, 0),
            Inst::abc(Op::Return, 1, 0, 0),
        ],
    );
    assert!(
        errors(&p)
            .iter()
            .any(|m| m.contains("`len` cannot be applied")),
        "{:?}",
        errors(&p)
    );
}
