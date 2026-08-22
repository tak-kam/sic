use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;

use super::*;

/// One hand-written function: name, parameter types, return type, register
/// count, and its instructions.
type FuncSpec<'a> = (&'a str, &'a [TypeTag], TypeTag, u8, Vec<Inst>);

/// Builds a program from hand-written functions.
///
/// The type section lists tags in tag order, so a `TypeTag` is its own index.
fn program(funcs: Vec<FuncSpec<'_>>, consts: Vec<Const>) -> Program {
    let mut p = Program {
        consts,
        types: vec![
            TypeTag::Unit,
            TypeTag::Bool,
            TypeTag::Int,
            TypeTag::Float,
            TypeTag::Str,
        ],
        ..Program::default()
    };
    for (name, params, ret, reg_count, code) in funcs {
        let code_off = p.code.len() as u32;
        p.code.extend(code);
        p.funcs.push(FuncDef {
            name: name.into(),
            params: params.iter().map(|t| *t as u32).collect(),
            reg_count,
            ret_type: ret as u32,
            code_off,
            code_len: p.code.len() as u32 - code_off,
        });
    }
    p
}

/// Runs the first function and returns its result, asserting it finished.
fn run(p: &Program, args: &[Value]) -> Value {
    let mut vm = Vm::new(p, DEFAULT_FUEL);
    match vm.run(0, args) {
        Status::Finished(v) => v,
        other => panic!("expected a result, got {other:?}"),
    }
}

fn fail_kind(p: &Program, args: &[Value]) -> FailKind {
    let mut vm = Vm::new(p, DEFAULT_FUEL);
    match vm.run(0, args) {
        Status::Failed(info) => info.kind,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn returns_a_constant() {
    let p = program(
        vec![(
            "main",
            &[],
            TypeTag::Int,
            1,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::Return, 0, 0, 0),
            ],
        )],
        vec![Const::I64(30)],
    );
    assert_eq!(run(&p, &[]), Value::I64(30));
}

#[test]
fn arithmetic() {
    // (a + b) * a
    let p = program(
        vec![(
            "f",
            &[TypeTag::Int, TypeTag::Int],
            TypeTag::Int,
            3,
            vec![
                Inst::abc(Op::AddI64, 2, 0, 1),
                Inst::abc(Op::MulI64, 2, 2, 0),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![],
    );
    assert_eq!(run(&p, &[Value::I64(3), Value::I64(4)]), Value::I64(21));
}

#[test]
fn overflow_and_division_by_zero_fail_rather_than_wrap() {
    let add = program(
        vec![(
            "f",
            &[TypeTag::Int, TypeTag::Int],
            TypeTag::Int,
            3,
            vec![
                Inst::abc(Op::AddI64, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![],
    );
    assert_eq!(
        fail_kind(&add, &[Value::I64(i64::MAX), Value::I64(1)]),
        FailKind::Overflow
    );

    let div = program(
        vec![(
            "f",
            &[TypeTag::Int, TypeTag::Int],
            TypeTag::Int,
            3,
            vec![
                Inst::abc(Op::DivI64, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![],
    );
    assert_eq!(
        fail_kind(&div, &[Value::I64(1), Value::I64(0)]),
        FailKind::DivisionByZero
    );
    // i64::MIN / -1 has no representable result either.
    assert_eq!(
        fail_kind(&div, &[Value::I64(i64::MIN), Value::I64(-1)]),
        FailKind::Overflow
    );
}

#[test]
fn comparison_and_branching() {
    // if a < b { return 1 } else { return 0 }
    let p = program(
        vec![(
            "f",
            &[TypeTag::Int, TypeTag::Int],
            TypeTag::Int,
            3,
            vec![
                Inst::abc(Op::Lt, 2, 0, 1),
                Inst::asbx(Op::JumpIfNot, 2, 2),
                Inst::abx(Op::LoadConst, 2, 0),
                Inst::abc(Op::Return, 2, 0, 0),
                Inst::abx(Op::LoadConst, 2, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![Const::I64(1), Const::I64(0)],
    );
    assert_eq!(run(&p, &[Value::I64(1), Value::I64(2)]), Value::I64(1));
    assert_eq!(run(&p, &[Value::I64(2), Value::I64(2)]), Value::I64(0));
}

#[test]
fn a_backward_jump_loops() {
    // sum = 0; while n > 0 { sum += n; n -= 1 }  return sum
    let p = program(
        vec![(
            "f",
            &[TypeTag::Int],
            TypeTag::Int,
            4,
            vec![
                Inst::abx(Op::LoadConst, 1, 0),  // sum = 0
                Inst::abx(Op::LoadConst, 2, 1),  // one = 1
                Inst::abx(Op::LoadConst, 3, 0),  // zero = 0
                Inst::abc(Op::Gt, 3, 0, 3),      // n > 0
                Inst::asbx(Op::JumpIfNot, 3, 4), // exit
                Inst::abc(Op::AddI64, 1, 1, 0),  // sum += n
                Inst::abc(Op::SubI64, 0, 0, 2),  // n -= 1
                Inst::abx(Op::LoadConst, 3, 0),  // zero again
                Inst::asbx(Op::Jump, 0, -6),     // back to the comparison
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::I64(0), Const::I64(1)],
    );
    assert_eq!(run(&p, &[Value::I64(4)]), Value::I64(10));
    assert_eq!(run(&p, &[Value::I64(0)]), Value::I64(0));
}

#[test]
fn calls_pass_arguments_and_return_values() {
    // main() { return add(2, 3) }   add(a, b) { return a + b }
    let p = program(
        vec![
            (
                "main",
                &[],
                TypeTag::Int,
                4,
                vec![
                    Inst::abx(Op::LoadConst, 2, 0),
                    Inst::abx(Op::LoadConst, 3, 1),
                    Inst::abc(Op::Call, 0, 1, 2),
                    Inst::abc(Op::Return, 0, 0, 0),
                ],
            ),
            (
                "add",
                &[TypeTag::Int, TypeTag::Int],
                TypeTag::Int,
                3,
                vec![
                    Inst::abc(Op::AddI64, 2, 0, 1),
                    Inst::abc(Op::Return, 2, 0, 0),
                ],
            ),
        ],
        vec![Const::I64(2), Const::I64(3)],
    );
    assert_eq!(run(&p, &[]), Value::I64(5));
}

#[test]
fn recursion_unwinds_correctly() {
    // countdown(n) { if n == 0 { return 0 } return countdown(n - 1) }
    let p = program(
        vec![(
            "countdown",
            &[TypeTag::Int],
            TypeTag::Int,
            4,
            vec![
                Inst::abx(Op::LoadConst, 1, 0), // zero
                Inst::abc(Op::Eq, 2, 0, 1),
                Inst::asbx(Op::JumpIfNot, 2, 1),
                Inst::abc(Op::Return, 1, 0, 0), // return 0
                Inst::abx(Op::LoadConst, 2, 1), // one
                Inst::abc(Op::SubI64, 3, 0, 2), // n - 1
                Inst::abc(Op::Call, 1, 0, 3),
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::I64(0), Const::I64(1)],
    );
    assert_eq!(run(&p, &[Value::I64(100)]), Value::I64(0));
    // Deep enough to hit the frame limit rather than the host stack.
    assert_eq!(
        fail_kind(&p, &[Value::I64(100_000)]),
        FailKind::CallStackTooDeep
    );
}

#[test]
fn fuel_runs_out_on_an_endless_loop() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeTag::Unit,
            1,
            vec![Inst::asbx(Op::Jump, 0, -1)],
        )],
        vec![],
    );
    let mut vm = Vm::new(&p, 1000);
    match vm.run(0, &[]) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::OutOfFuel),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(vm.fuel(), 0);
}

#[test]
fn fail_reports_its_value_and_position() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeTag::Unit,
            1,
            vec![Inst::abx(Op::LoadConst, 0, 0), Inst::abc(Op::Fail, 0, 0, 0)],
        )],
        vec![Const::Str("boom".into())],
    );
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::Explicit);
            assert_eq!(info.func, "f");
            assert_eq!(info.pc, 1);
            assert_eq!(vm.display(&info.value.unwrap()), "\"boom\"");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn strings_compare_by_content() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeTag::Bool,
            3,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abx(Op::LoadConst, 1, 1),
                Inst::abc(Op::Eq, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        // Two separate constants that happen to hold the same text.
        vec![Const::Str("same".into()), Const::Str("same".into())],
    );
    assert_eq!(run(&p, &[]), Value::Bool(true));
}

#[test]
fn halt_ends_the_run() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeTag::Unit,
            1,
            vec![Inst::abc(Op::Halt, 0, 0, 0)],
        )],
        vec![],
    );
    assert_eq!(run(&p, &[]), Value::Unit);
}

#[test]
fn unverified_bytecode_fails_instead_of_panicking() {
    // A constant index nothing checked: the VM must not index out of bounds.
    let p = program(
        vec![(
            "f",
            &[],
            TypeTag::Int,
            1,
            vec![
                Inst::abx(Op::LoadConst, 0, 99),
                Inst::abc(Op::Return, 0, 0, 0),
            ],
        )],
        vec![],
    );
    assert!(matches!(fail_kind(&p, &[]), FailKind::Internal(_)));
}

// ---- capabilities ----

/// The same program in each capability test: `process.exec("/usr/bin/true")`,
/// whose result is returned.
fn exec_program() -> Program {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeTag::Int,
            2,
            vec![
                Inst::abx(Op::LoadConst, 1, 0),
                Inst::abc(Op::CallCap, 0, 0, 1),
                Inst::abc(Op::Return, 0, 0, 0),
            ],
        )],
        vec![Const::Str("/usr/bin/true".into())],
    );
    p.caps.push(sic_bytecode::CapDecl {
        name: "process.exec".into(),
        kind: sic_core::CapKind::Exec,
        constraints: "/usr/bin/true".into(),
        params: vec![TypeTag::Str as u32],
        ret_type: TypeTag::Int as u32,
    });
    p
}

#[test]
fn a_capability_call_suspends_instead_of_performing_the_effect() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    let Status::Suspended(request) = vm.run(0, &[]) else {
        panic!("the VM should have suspended");
    };
    assert_eq!(request.index, 0);
    assert_eq!(request.name, "process.exec");
    // Arguments cross the boundary as owned values, not handles into the arena.
    assert_eq!(
        request.args,
        vec![sic_core::CapValue::Str("/usr/bin/true".into())]
    );
    assert!(vm.is_suspended());
}

#[test]
fn resuming_writes_the_result_and_continues() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    match vm.resume(sic_core::CapValue::I64(0)) {
        Status::Finished(v) => assert_eq!(v, Value::I64(0)),
        other => panic!("expected a result, got {other:?}"),
    }
    assert!(!vm.is_suspended());
}

#[test]
fn a_returned_string_is_interned_in_the_arena() {
    let mut p = exec_program();
    // Make the capability return a String instead.
    p.caps[0].ret_type = TypeTag::Str as u32;
    p.funcs[0].ret_type = TypeTag::Str as u32;

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    match vm.resume(sic_core::CapValue::Str("contents".into())) {
        Status::Finished(v) => assert_eq!(vm.display(&v), "\"contents\""),
        other => panic!("expected a result, got {other:?}"),
    }
}

#[test]
fn a_failed_capability_ends_the_run() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    match vm.resume_failed(&sic_core::CapError::new("permission denied")) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::Capability);
            assert_eq!(info.detail.as_deref(), Some("permission denied"));
            assert_eq!(info.pc, 1);
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn resuming_a_running_vm_is_an_internal_error() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.resume(sic_core::CapValue::I64(0)) {
        Status::Failed(info) => assert!(matches!(info.kind, FailKind::Internal(_))),
        other => panic!("expected a failure, got {other:?}"),
    }
}

// ---- the execution journal ----

use std::cell::RefCell;
use std::rc::Rc;

use sic_journal::{Event, EventKind, Journal, RunId, Sink};

/// A sink that stays readable after the journal takes ownership of it.
#[derive(Debug, Clone, Default)]
struct SharedSink(Rc<RefCell<Vec<Event>>>);

impl Sink for SharedSink {
    fn emit(&mut self, event: &Event) {
        self.0.borrow_mut().push(event.clone());
    }
}

fn journal_for(sink: &SharedSink) -> Journal {
    Journal::new(RunId(42), Box::new(sink.clone()))
}

fn names(sink: &SharedSink) -> Vec<&'static str> {
    sink.0.borrow().iter().map(|e| e.kind.name()).collect()
}

#[test]
fn a_run_records_its_shape() {
    // main() { return add(2, 3) } over two functions, so the journal has to
    // show one call nested inside the other.
    let p = program(
        vec![
            (
                "main",
                &[],
                TypeTag::Int,
                4,
                vec![
                    Inst::abx(Op::LoadConst, 2, 0),
                    Inst::abx(Op::LoadConst, 3, 1),
                    Inst::abc(Op::Call, 0, 1, 2),
                    Inst::abc(Op::Return, 0, 0, 0),
                ],
            ),
            (
                "add",
                &[TypeTag::Int, TypeTag::Int],
                TypeTag::Int,
                3,
                vec![
                    Inst::abc(Op::AddI64, 2, 0, 1),
                    Inst::abc(Op::Return, 2, 0, 0),
                ],
            ),
        ],
        vec![Const::I64(2), Const::I64(3)],
    );
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Finished(_)));

    assert_eq!(
        names(&sink),
        vec![
            "run_started",
            "function_entered", // main
            "function_entered", // add
            "function_exited",  // add
            "function_exited",  // main
            "run_completed",
        ]
    );

    let events = sink.0.borrow();
    // The trace shape is recorded as it happens: add sits inside main, which
    // sits inside the run.
    let run_span = events[0].span;
    let main_span = events[1].span;
    let add_span = events[2].span;
    assert_eq!(events[0].parent, None);
    assert_eq!(events[1].parent, Some(run_span));
    assert_eq!(events[2].parent, Some(main_span));
    assert_ne!(add_span, main_span);

    // Sequence numbers are the order, and they are dense.
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5]);
    assert!(events.iter().all(|e| e.run == RunId(42)));
}

#[test]
fn a_failure_is_recorded_with_its_reason() {
    let p = program(
        vec![(
            "f",
            &[TypeTag::Int, TypeTag::Int],
            TypeTag::Int,
            3,
            vec![
                Inst::abc(Op::DivI64, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![],
    );
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(
        vm.run(0, &[Value::I64(1), Value::I64(0)]),
        Status::Failed(_)
    ));

    assert_eq!(names(&sink).last(), Some(&"run_failed"));
    let events = sink.0.borrow();
    let EventKind::RunFailed { error } = &events.last().unwrap().kind else {
        panic!("expected a failure");
    };
    assert!(error.contains("division by zero"), "{error}");
}

#[test]
fn a_capability_call_is_recorded_at_both_ends() {
    let p = exec_program();
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    assert_eq!(names(&sink).last(), Some(&"capability_requested"));

    assert!(matches!(
        vm.resume(sic_core::CapValue::I64(0)),
        Status::Finished(_)
    ));
    assert_eq!(
        names(&sink),
        vec![
            "run_started",
            "function_entered",
            "capability_requested",
            "capability_completed",
            "function_exited",
            "run_completed",
        ]
    );

    let events = sink.0.borrow();
    // The capability span sits inside the function that called it, and the
    // request and its completion share that span.
    assert_eq!(events[2].parent, Some(events[1].span));
    assert_eq!(events[3].span, events[2].span);
}

#[test]
fn a_failed_capability_is_recorded_before_the_run_fails() {
    let p = exec_program();
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    assert!(matches!(
        vm.resume_failed(&sic_core::CapError::new("permission denied")),
        Status::Failed(_)
    ));
    assert_eq!(
        names(&sink),
        vec![
            "run_started",
            "function_entered",
            "capability_requested",
            "capability_failed",
            "run_failed",
        ]
    );
}

#[test]
fn arguments_are_recorded_as_digests_not_values() {
    // A capability argument is exactly the kind of value that must not reach
    // telemetry by default.
    let p = exec_program();
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));

    let events = sink.0.borrow();
    let rendered: Vec<String> = events
        .iter()
        .map(sic_journal::json::event_to_json)
        .collect();
    let all = rendered.join("\n");
    assert!(!all.contains("/usr/bin/true"), "{all}");
    assert!(all.contains("sha256:"), "{all}");
}

// ---- checkpoints ----

use sic_core::Digest;

fn program_digest() -> Digest {
    Digest::of(b"the program these checkpoints belong to")
}

#[test]
fn a_suspended_run_survives_being_written_out_and_read_back() {
    let p = exec_program();

    // First process: run until the capability, then write the state out.
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm
        .checkpoint(program_digest(), "may I run /usr/bin/true?")
        .expect("a suspended run can be checkpointed");
    drop(vm);

    // Second process: pick it up and answer.
    let resumed_sink = SharedSink::default();
    let (mut vm, question) =
        Vm::restore(&p, &bytes, program_digest(), Box::new(resumed_sink.clone()))
            .expect("the checkpoint should restore");
    assert_eq!(question, "may I run /usr/bin/true?");
    assert!(vm.is_suspended());

    match vm.resume(sic_core::CapValue::I64(7)) {
        Status::Finished(v) => assert_eq!(v, Value::I64(7)),
        other => panic!("expected a result, got {other:?}"),
    }
}

#[test]
fn the_journal_continues_across_the_checkpoint() {
    // A resumed run is the same run, so its events are one sequence.
    let p = exec_program();
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();

    assert_eq!(
        names(&sink),
        vec![
            "run_started",
            "function_entered",
            "capability_requested",
            "run_suspended",
            "checkpoint_written",
        ]
    );
    let first_seqs: Vec<u64> = sink.0.borrow().iter().map(|e| e.seq).collect();
    assert_eq!(first_seqs, vec![0, 1, 2, 3, 4]);

    let second = SharedSink::default();
    let (mut vm, _) = Vm::restore(&p, &bytes, program_digest(), Box::new(second.clone())).unwrap();
    assert!(matches!(
        vm.resume(sic_core::CapValue::I64(0)),
        Status::Finished(_)
    ));

    assert_eq!(
        names(&second),
        vec![
            "run_resumed",
            "capability_completed",
            "function_exited",
            "run_completed",
        ]
    );
    // No sequence number is reused, and the run id is the same.
    let second_seqs: Vec<u64> = second.0.borrow().iter().map(|e| e.seq).collect();
    assert_eq!(second_seqs, vec![5, 6, 7, 8]);
    assert!(second.0.borrow().iter().all(|e| e.run == RunId(42)));
}

#[test]
fn strings_survive_a_checkpoint() {
    // Handles only mean anything against their arena, so the arena travels with
    // the registers.
    let mut p = exec_program();
    p.caps[0].ret_type = TypeTag::Str as u32;
    p.funcs[0].ret_type = TypeTag::Str as u32;

    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();

    let (mut vm, _) = Vm::restore(
        &p,
        &bytes,
        program_digest(),
        Box::new(SharedSink::default()),
    )
    .unwrap();
    match vm.resume(sic_core::CapValue::Str("answer".into())) {
        Status::Finished(v) => assert_eq!(vm.display(&v), "\"answer\""),
        other => panic!("expected a result, got {other:?}"),
    }
}

#[test]
fn a_checkpoint_cannot_be_resumed_against_other_bytecode() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();

    let err = Vm::restore(
        &p,
        &bytes,
        Digest::of(b"a different program"),
        Box::new(SharedSink::default()),
    )
    .unwrap_err();
    assert!(err.message.contains("different bytecode"), "{err}");
}

#[test]
fn a_running_vm_has_no_checkpoint() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(vm.checkpoint(program_digest(), "?").is_none());
}

#[test]
fn a_corrupt_checkpoint_is_refused_rather_than_restored() {
    let p = exec_program();
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();

    // Truncated at every length: a short read must never become a bad VM.
    for cut in [0, 4, 8, 40, 60, bytes.len() - 1] {
        assert!(
            Vm::restore(
                &p,
                &bytes[..cut],
                program_digest(),
                Box::new(SharedSink::default())
            )
            .is_err(),
            "a checkpoint cut at {cut} should be refused"
        );
    }

    // A wrong magic.
    let mut broken = bytes.clone();
    broken[0] = b'X';
    assert!(
        Vm::restore(
            &p,
            &broken,
            program_digest(),
            Box::new(SharedSink::default())
        )
        .is_err()
    );
}

#[test]
fn a_checkpoint_pointing_outside_its_own_state_is_refused() {
    use crate::checkpoint::{Checkpoint, Frame, Pending};

    let base = Checkpoint {
        program_digest: program_digest(),
        run: 1,
        seq: 0,
        next_span: 0,
        fuel: 10,
        pending: Pending {
            reg: 0,
            cap: "process.exec".into(),
            span: 0,
            parent: None,
            question: String::new(),
        },
        frames: vec![Frame {
            func: 0,
            pc: 0,
            reg_base: 0,
            ret_reg: 0,
            span: 0,
            parent: None,
        }],
        regs: vec![Value::Unit],
        str_consts: vec![None],
        strings: Vec::new(),
    };
    // The honest one decodes.
    assert!(Checkpoint::decode(&base.encode()).is_ok());

    // A pending call writing to a register that does not exist.
    let mut bad = base.clone();
    bad.pending.reg = 9;
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // A value pointing outside the saved arena.
    let mut bad = base.clone();
    bad.regs = vec![Value::Str(Handle(3))];
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // No frames at all.
    let mut bad = base.clone();
    bad.frames.clear();
    assert!(Checkpoint::decode(&bad.encode()).is_err());
}

#[test]
fn a_checkpoint_frame_must_point_into_this_program() {
    use crate::checkpoint::{Checkpoint, Frame, Pending};

    let p = exec_program();
    let checkpoint = Checkpoint {
        program_digest: program_digest(),
        run: 1,
        seq: 0,
        next_span: 0,
        fuel: 10,
        pending: Pending {
            reg: 0,
            cap: "process.exec".into(),
            span: 0,
            parent: None,
            question: String::new(),
        },
        frames: vec![Frame {
            func: 0,
            // Past the end of main.
            pc: 900,
            reg_base: 0,
            ret_reg: 0,
            span: 0,
            parent: None,
        }],
        regs: vec![Value::Unit],
        str_consts: vec![None],
        strings: Vec::new(),
    };
    let err = Vm::restore(
        &p,
        &checkpoint.encode(),
        program_digest(),
        Box::new(SharedSink::default()),
    )
    .unwrap_err();
    assert!(err.message.contains("outside"), "{err}");
}
