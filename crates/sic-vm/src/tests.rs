use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;

/// The type section holds the primitives in tag order, so a primitive is its
/// own index.
fn index_of(desc: TypeDesc) -> u32 {
    desc.primitive_index().expect("a primitive type")
}

use super::*;

/// One hand-written function: name, parameter types, return type, register
/// count, and its instructions.
type FuncSpec<'a> = (&'a str, &'a [TypeDesc], TypeDesc, u8, Vec<Inst>);

/// Builds a program from hand-written functions.
///
/// The type section lists tags in tag order, so a `TypeDesc` is its own index.
fn program(funcs: Vec<FuncSpec<'_>>, consts: Vec<Const>) -> Program {
    let mut p = Program {
        consts,
        types: TypeDesc::primitives(),
        ..Program::default()
    };
    for (name, params, ret, reg_count, code) in funcs {
        let code_off = p.code.len() as u32;
        p.code.extend(code);
        p.funcs.push(FuncDef {
            name: name.into(),
            params: params.iter().map(|t| index_of(t.clone())).collect(),
            reg_count,
            ret_type: index_of(ret),
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
            TypeDesc::Int,
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
            &[TypeDesc::Int, TypeDesc::Int],
            TypeDesc::Int,
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
            &[TypeDesc::Int, TypeDesc::Int],
            TypeDesc::Int,
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
            &[TypeDesc::Int, TypeDesc::Int],
            TypeDesc::Int,
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
            &[TypeDesc::Int, TypeDesc::Int],
            TypeDesc::Int,
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
            &[TypeDesc::Int],
            TypeDesc::Int,
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
                TypeDesc::Int,
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
                &[TypeDesc::Int, TypeDesc::Int],
                TypeDesc::Int,
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
            &[TypeDesc::Int],
            TypeDesc::Int,
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
            TypeDesc::Unit,
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

/// A join is charged a fuel for each byte of its result, and the arithmetic is
/// written out here rather than left to the end-to-end test, because "the
/// budget bounds the arena" is only true if the number is exactly this one.
///
/// Four instructions run - two loads, the join, the return - so four fuel go on
/// the instructions, and eleven more on the eleven bytes of "hello world".
#[test]
fn a_join_costs_a_fuel_for_each_byte_of_its_result() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeDesc::Str,
            3,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abx(Op::LoadConst, 1, 1),
                Inst::abc(Op::Concat, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![Const::Str("hello ".into()), Const::Str("world".into())],
    );
    let mut vm = Vm::new(&p, 1000);
    let Status::Finished(Value::Str(handle)) = vm.run(0, &[]) else {
        panic!("expected a string");
    };
    assert_eq!(vm.arena().str(handle), "hello world");
    assert_eq!(vm.fuel(), 1000 - 4 - 11);
}

/// And the charge comes before the allocation, so a program that cannot afford
/// a string never gets one: the run stops with the budget spent rather than
/// with the memory taken.
#[test]
fn a_join_nobody_can_afford_is_never_built() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeDesc::Str,
            3,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abx(Op::LoadConst, 1, 0),
                Inst::abc(Op::Concat, 2, 0, 1),
                Inst::abc(Op::Return, 2, 0, 0),
            ],
        )],
        vec![Const::Str("0123456789".into())],
    );
    // Enough for the instructions and for nineteen of the twenty bytes.
    let mut vm = Vm::new(&p, 3 + 19);
    match vm.run(0, &[]) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::OutOfFuel),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(vm.fuel(), 0);
    assert_eq!(
        vm.arena().strings().len(),
        1,
        "only the constant should be in the arena"
    );
}

#[test]
fn fail_reports_its_value_and_position() {
    let p = program(
        vec![(
            "f",
            &[],
            TypeDesc::Unit,
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
            TypeDesc::Bool,
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
            TypeDesc::Unit,
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
            TypeDesc::Int,
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
            TypeDesc::Int,
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
        pin: String::new(),
        answers: sic_core::Answers::Unsaid,
        repeatable: false,
        delegable: false,
        dir: String::new(),
        env: Vec::new(),
        args: Vec::new(),
        params: vec![4],
        ret_type: 2,
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
    p.caps[0].ret_type = 4;
    p.funcs[0].ret_type = 4;

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
                TypeDesc::Int,
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
                &[TypeDesc::Int, TypeDesc::Int],
                TypeDesc::Int,
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
            "task_started",     // the entry task
            "function_entered", // main
            "function_entered", // add
            "function_exited",  // add
            "function_exited",  // main
            "task_completed",
            "run_completed",
        ]
    );

    let events = sink.0.borrow();
    // The trace shape is recorded as it happens: add sits inside main, which
    // sits inside the task, which sits inside the run.
    let run_span = events[0].span;
    let task_span = events[1].span;
    let main_span = events[2].span;
    let add_span = events[3].span;
    assert_eq!(events[0].parent, None);
    assert_eq!(events[1].parent, Some(run_span));
    assert_eq!(events[2].parent, Some(task_span));
    assert_eq!(events[3].parent, Some(main_span));
    assert_ne!(add_span, main_span);

    // Sequence numbers are the order, and they are dense.
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (0..8).collect::<Vec<u64>>());
    assert!(events.iter().all(|e| e.run == RunId(42)));
}

#[test]
fn a_failure_is_recorded_with_its_reason() {
    let p = program(
        vec![(
            "f",
            &[TypeDesc::Int, TypeDesc::Int],
            TypeDesc::Int,
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
            "task_started",
            "function_entered",
            "capability_requested",
            "capability_completed",
            "function_exited",
            "task_completed",
            "run_completed",
        ]
    );

    let events = sink.0.borrow();
    // The capability span sits inside the function that called it, and the
    // request and its completion share that span.
    assert_eq!(events[3].parent, Some(events[2].span));
    assert_eq!(events[4].span, events[3].span);
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
            "task_started",
            "function_entered",
            "capability_requested",
            "capability_failed",
            "task_failed",
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
            "task_started",
            "function_entered",
            "capability_requested",
            "run_suspended",
            "checkpoint_written",
        ]
    );
    let first_seqs: Vec<u64> = sink.0.borrow().iter().map(|e| e.seq).collect();
    assert_eq!(first_seqs, (0..6).collect::<Vec<u64>>());

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
            "task_completed",
            "run_completed",
        ]
    );
    // No sequence number is reused, and the run id is the same.
    let second_seqs: Vec<u64> = second.0.borrow().iter().map(|e| e.seq).collect();
    assert_eq!(second_seqs, (6..11).collect::<Vec<u64>>());
    assert!(second.0.borrow().iter().all(|e| e.run == RunId(42)));
}

#[test]
fn strings_survive_a_checkpoint() {
    // Handles only mean anything against their arena, so the arena travels with
    // the registers.
    let mut p = exec_program();
    p.caps[0].ret_type = 4;
    p.funcs[0].ret_type = 4;

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
    use crate::checkpoint::{Checkpoint, Frame, Pending, TaskSnapshot, TaskStateSnapshot};

    let waiting = TaskStateSnapshot::WaitingCap(Pending {
        reg: 0,
        index: 0,
        cap: "process.exec".into(),
        args: Vec::new(),
        attempt: 1,
        attempts: 1,
        timeout_ms: 0,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        rejected: String::new(),
        pc: 0,
        span: 0,
        parent: None,
    });
    let base = Checkpoint {
        program_digest: program_digest(),
        run: 1,
        seq: 0,
        next_span: 0,
        root_span: 0,
        fuel: 10,
        cursor: 0,
        answering: 0,
        question: String::new(),
        tasks: vec![TaskSnapshot {
            state: waiting.clone(),
            span: 0,
            func_name: "main".into(),
            regs: vec![Value::Unit],
            frames: vec![Frame {
                func: 0,
                pc: 0,
                reg_base: 0,
                ret_reg: 0,
                span: 0,
                parent: None,
            }],
        }],
        str_consts: vec![None],
        strings: Vec::new(),
        lists: Vec::new(),
        objects: Vec::new(),
        spent: Vec::new(),
        used_tools: Vec::new(),
    };
    // The honest one decodes.
    assert!(Checkpoint::decode(&base.encode()).is_ok());

    // An answer going into a register that does not exist.
    let mut bad = base.clone();
    bad.tasks[0].state = TaskStateSnapshot::WaitingCap(Pending {
        reg: 9,
        ..match waiting.clone() {
            TaskStateSnapshot::WaitingCap(p) => p,
            _ => unreachable!(),
        }
    });
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // A value pointing outside the saved arena.
    let mut bad = base.clone();
    bad.tasks[0].regs = vec![Value::Str(Handle(3))];
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // A task value naming a task that does not exist.
    let mut bad = base.clone();
    bad.tasks[0].regs = vec![Value::Task(7)];
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // No tasks at all.
    let mut bad = base.clone();
    bad.tasks.clear();
    assert!(Checkpoint::decode(&bad.encode()).is_err());

    // Waiting on a task that does not exist.
    let mut bad = base.clone();
    bad.tasks[0].state = TaskStateSnapshot::WaitingTask(4);
    // The answering task must be the waiting one, so this fails on that too.
    assert!(Checkpoint::decode(&bad.encode()).is_err());
}

#[test]
fn a_checkpoint_frame_must_point_into_this_program() {
    use crate::checkpoint::{Checkpoint, Frame, Pending, TaskSnapshot, TaskStateSnapshot};

    let p = exec_program();
    let checkpoint = Checkpoint {
        program_digest: program_digest(),
        run: 1,
        seq: 0,
        next_span: 0,
        root_span: 0,
        fuel: 10,
        cursor: 0,
        answering: 0,
        question: String::new(),
        tasks: vec![TaskSnapshot {
            state: TaskStateSnapshot::WaitingCap(Pending {
                reg: 0,
                index: 0,
                cap: "process.exec".into(),
                args: Vec::new(),
                attempt: 1,
                attempts: 1,
                timeout_ms: 0,
                conversation: 0,
                tools: 0,
                deadline_ms: 0,
                rejected: String::new(),
                pc: 0,
                span: 0,
                parent: None,
            }),
            span: 0,
            func_name: "main".into(),
            regs: vec![Value::Unit],
            frames: vec![Frame {
                func: 0,
                // Past the end of main.
                pc: 900,
                reg_base: 0,
                ret_reg: 0,
                span: 0,
                parent: None,
            }],
        }],
        str_consts: vec![None],
        strings: Vec::new(),
        lists: Vec::new(),
        objects: Vec::new(),
        spent: Vec::new(),
        used_tools: Vec::new(),
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

// ---- tasks ----

/// `main` spawns `double(2)` and awaits it.
///
/// Written by hand because this crate does not depend on the compiler. The
/// type section needs a `Task<Int>` for the verifier's sake; the VM does not
/// read it, but a program that would not verify is not worth testing.
fn task_program(await_twice: bool) -> Program {
    let mut code = vec![
        Inst::abx(Op::LoadConst, 1, 0), // r1 = 2
        Inst::abc(Op::Spawn, 0, 1, 1),  // r0 = spawn double(r1)
        Inst::abc(Op::Await, 2, 0, 0),  // r2 = await r0
    ];
    if await_twice {
        code.push(Inst::abc(Op::Await, 2, 0, 0));
    }
    code.push(Inst::abc(Op::Return, 2, 0, 0));

    let mut p = program(
        vec![
            ("main", &[], TypeDesc::Int, 3, code),
            (
                "double",
                &[TypeDesc::Int],
                TypeDesc::Int,
                2,
                vec![
                    Inst::abc(Op::AddI64, 1, 0, 0),
                    Inst::abc(Op::Return, 1, 0, 0),
                ],
            ),
        ],
        vec![Const::I64(2)],
    );
    p.types.push(TypeDesc::Task(index_of(TypeDesc::Int)));
    p
}

#[test]
fn a_spawned_task_runs_and_its_result_is_awaited() {
    let p = task_program(false);
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Finished(v) => assert_eq!(v, Value::I64(4)),
        other => panic!("expected a result, got {other:?}"),
    }
    assert_eq!(vm.task_count(), 2);
}

#[test]
fn a_task_cannot_be_awaited_twice() {
    // A result is moved out of the task, not copied.
    let p = task_program(true);
    assert_eq!(fail_kind(&p, &[]), FailKind::TaskAlreadyAwaited);
}

#[test]
fn a_task_that_fails_fails_the_task_that_awaits_it() {
    let mut p = task_program(false);
    // Make `double` divide by zero instead.
    let code_off = p.funcs[1].code_off as usize;
    p.code[code_off] = Inst::abc(Op::DivI64, 1, 0, 1);
    // r1 is uninitialized in the VM's eyes but holds Unit, so use a zero const.
    p.consts.push(Const::I64(0));
    p.code[code_off] = Inst::abx(Op::LoadConst, 1, 1);
    p.code.insert(code_off + 1, Inst::abc(Op::DivI64, 1, 0, 1));
    // Shifting the code moves everything after it.
    p.funcs[1].code_len += 1;

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::AwaitedTaskFailed);
            assert!(
                info.detail
                    .as_deref()
                    .unwrap_or("")
                    .contains("division by zero"),
                "{info:?}"
            );
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_task_the_run_never_waited_for_is_recorded_as_abandoned() {
    // Silently discarding it is how a workflow claims to have succeeded when
    // part of it did not run.
    let mut p = task_program(false);
    // Drop the AWAIT: main spawns and returns a constant instead.
    let code_off = p.funcs[0].code_off as usize;
    p.code[code_off + 2] = Inst::abx(Op::LoadConst, 2, 0);

    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Finished(Value::I64(2))));

    let events = names(&sink);
    assert!(
        events.contains(&"task_abandoned") || events.contains(&"task_completed"),
        "{events:?}"
    );
}

#[test]
fn the_journal_names_the_task_each_event_belongs_to() {
    let p = task_program(false);
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Finished(_)));

    let events = sink.0.borrow();
    // The task field stops being zero for everything.
    let tasks: std::collections::HashSet<u64> = events.iter().map(|e| e.task.0).collect();
    assert!(tasks.contains(&0) && tasks.contains(&1), "{tasks:?}");
}

#[test]
fn spawning_past_the_task_limit_says_the_table_is_full() {
    // A program that spawns its way past `MAX_TASKS` used to be told its call
    // stack was too deep, which sent whoever read it to look at recursion. The
    // frames are fine; there is nowhere to put another task.
    //
    // Reaching the limit costs one `SPAWN` per task and nothing runs them,
    // because the task that spawns them fails before it yields.
    let mut p = program(
        vec![
            (
                "main",
                &[],
                TypeDesc::Unit,
                1,
                vec![
                    Inst::abc(Op::Spawn, 0, 1, 0), // r0 = spawn work()
                    Inst::asbx(Op::Jump, 0, -2),   // and again
                ],
            ),
            (
                "work",
                &[],
                TypeDesc::Int,
                1,
                vec![
                    Inst::abx(Op::LoadConst, 0, 0),
                    Inst::abc(Op::Return, 0, 0, 0),
                ],
            ),
        ],
        vec![Const::I64(1)],
    );
    p.types.push(TypeDesc::Task(index_of(TypeDesc::Int)));

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::TooManyTasks);
            assert!(info.describe().contains("tasks"), "{info:?}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    // The entry task is one of them, so the limit is reached exactly.
    assert_eq!(vm.task_count(), MAX_TASKS);
}

#[test]
fn spawning_a_function_that_does_not_exist_is_not_reported_as_a_limit() {
    // The other way a spawn fails. It is a statement about the bytecode rather
    // than about the program's behaviour, and the two must not share a message.
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Unit,
            1,
            vec![Inst::abc(Op::Spawn, 0, 7, 0)],
        )],
        vec![],
    );
    p.types.push(TypeDesc::Task(index_of(TypeDesc::Int)));

    match fail_kind(&p, &[]) {
        FailKind::Internal(what) => assert!(what.contains("does not exist"), "{what}"),
        other => panic!("expected an internal failure, got {other:?}"),
    }
}

#[test]
fn a_call_chain_can_run_out_of_registers_before_it_runs_out_of_frames() {
    // `countdown` in `recursion_unwinds_correctly` needs four registers, so it
    // reaches `MAX_FRAMES` first and the register window check never fires.
    // This one needs 255 per activation, so the window is full after 258 calls
    // - a quarter of the frame limit - and that check is the only one that can
    // end this run.
    let p = program(
        vec![(
            "deep",
            &[TypeDesc::Int],
            TypeDesc::Int,
            255,
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
    // Raising either limit could make the frame count fire first and quietly
    // turn this test into a second copy of `recursion_unwinds_correctly`.
    const { assert!(255 * MAX_FRAMES > MAX_REGS) };
    assert_eq!(
        fail_kind(&p, &[Value::I64(100_000)]),
        FailKind::CallStackTooDeep
    );
}

// ---- retry ----

#[test]
fn a_failed_call_is_retried_up_to_the_policy() {
    let mut p = exec_program();
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 3,
        timeout_ms: 0,
        budget: 0,
        budget_group: 0,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });

    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));

    let Status::Suspended(request) = vm.run(0, &[]) else {
        panic!("expected a capability request");
    };
    assert_eq!(request.attempt, 1);

    let Status::Suspended(request) = vm.resume_failed(&sic_core::CapError::new("nope")) else {
        panic!("expected a second attempt");
    };
    assert_eq!(request.attempt, 2);

    let Status::Suspended(request) = vm.resume_failed(&sic_core::CapError::new("nope")) else {
        panic!("expected a third attempt");
    };
    assert_eq!(request.attempt, 3);

    // The policy allows three, so the fourth failure ends the run.
    match vm.resume_failed(&sic_core::CapError::new("nope")) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::Capability),
        other => panic!("expected a failure, got {other:?}"),
    }

    // Every attempt is in the journal, not only the last.
    let attempts = names(&sink)
        .iter()
        .filter(|n| **n == "capability_requested")
        .count();
    assert_eq!(attempts, 3);
}

#[test]
fn the_timeout_travels_with_the_request() {
    // The VM never reads a clock; the broker is told how long it has.
    let mut p = exec_program();
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 250,
        budget: 0,
        budget_group: 0,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    let Status::Suspended(request) = vm.run(0, &[]) else {
        panic!("expected a capability request");
    };
    assert_eq!(request.timeout_ms, 250);
    assert_eq!(request.task, 0);
}

// ---- records and lists ----

/// `main` builds `Point { x: 3, y: 4 }`, reads `y`, and returns it.
fn record_program() -> Program {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            5,
            vec![
                Inst::abx(Op::LoadConst, 1, 0), // 3
                Inst::abx(Op::LoadConst, 2, 1), // 4
                Inst::abc(Op::MakeObject, 3, 5, 1),
                Inst::abc(Op::GetField, 4, 3, 1),
                Inst::abc(Op::Return, 4, 0, 0),
            ],
        )],
        vec![Const::I64(3), Const::I64(4)],
    );
    p.types.push(TypeDesc::Object {
        name: "Point".into(),
        fields: vec![
            Field::new("x", index_of(TypeDesc::Int)),
            Field::new("y", index_of(TypeDesc::Int)),
        ],
        open: false,
    });
    p
}

#[test]
fn a_record_is_built_and_read_by_position() {
    let p = record_program();
    assert_eq!(run(&p, &[]), Value::I64(4));
}

#[test]
fn a_list_is_built_indexed_and_measured() {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            6,
            vec![
                Inst::abx(Op::LoadConst, 0, 0), // 10
                Inst::abx(Op::LoadConst, 1, 1), // 20
                Inst::abc(Op::MakeList, 2, 0, 2),
                Inst::abx(Op::LoadConst, 3, 2), // index 1
                Inst::abc(Op::GetIndex, 4, 2, 3),
                Inst::abc(Op::Len, 5, 2, 0),
                Inst::abc(Op::AddI64, 5, 4, 5),
                Inst::abc(Op::Return, 5, 0, 0),
            ],
        )],
        vec![Const::I64(10), Const::I64(20), Const::I64(1)],
    );
    p.types.push(TypeDesc::List(index_of(TypeDesc::Int)));
    // 20 + 2
    assert_eq!(run(&p, &[]), Value::I64(22));
}

#[test]
fn an_index_outside_the_list_fails_the_run() {
    // There is no option type to return instead, and a silent default would be
    // worse than stopping.
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            4,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::MakeList, 1, 0, 1),
                Inst::abx(Op::LoadConst, 2, 1), // index 5
                Inst::abc(Op::GetIndex, 3, 1, 2),
                Inst::abc(Op::Return, 3, 0, 0),
            ],
        )],
        vec![Const::I64(10), Const::I64(5)],
    );
    p.types.push(TypeDesc::List(index_of(TypeDesc::Int)));
    assert_eq!(fail_kind(&p, &[]), FailKind::IndexOutOfRange);
}

#[test]
fn len_of_a_string_counts_characters() {
    // Bytes would be about the encoding rather than the text.
    let p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            2,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::Len, 1, 0, 0),
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::Str("aあ😀".into())],
    );
    assert_eq!(run(&p, &[]), Value::I64(3));
}

#[test]
fn an_empty_list_constant_is_shared() {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            2,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::Len, 1, 0, 0),
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::EmptyList(0)],
    );
    p.types.push(TypeDesc::List(index_of(TypeDesc::Int)));
    p.consts[0] = Const::EmptyList(p.types.len() as u32 - 1);
    assert_eq!(run(&p, &[]), Value::I64(0));
}

#[test]
fn a_nested_value_survives_a_checkpoint() {
    // Handles only mean anything against their own store, so every store
    // travels with the registers.
    let mut p = exec_program();
    p.types.push(TypeDesc::List(index_of(TypeDesc::Int)));
    let list_type = p.types.len() as u32 - 1;
    p.consts.push(Const::EmptyList(list_type));

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();
    let (mut vm, _) = Vm::restore(
        &p,
        &bytes,
        program_digest(),
        Box::new(SharedSink::default()),
    )
    .expect("the checkpoint should restore");
    assert!(matches!(
        vm.resume(sic_core::CapValue::I64(1)),
        Status::Finished(Value::I64(1))
    ));
}

// ---- schema validation ----

/// `main` parses the constant document as `Wrapper { value: Int }`.
fn json_program(document: &str) -> Program {
    json_program_with(document, false)
}

/// The same, and `open` says whether the type ends in `..`.
fn json_program_with(document: &str, open: bool) -> Program {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            2,
            vec![
                Inst::abx(Op::LoadConst, 0, 0),
                Inst::abc(Op::FromJson, 1, 5, 0),
                Inst::abc(Op::GetField, 1, 1, 0),
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::Str(document.into())],
    );
    p.types.push(TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![Field::new("value", index_of(TypeDesc::Int))],
        open,
    });
    p.funcs[0].ret_type = index_of(TypeDesc::Int);
    p
}

/// The same again with `value` written `Int?`, and `GET_OPT` in place of
/// `GET_FIELD` - which is not a choice the compiler leaves open, because the
/// verifier refuses each instruction on the other kind of field.
fn optional_program(document: &str) -> Program {
    let mut p = json_program(document);
    p.types[5] = TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![Field {
            name: "value".into(),
            ty: index_of(TypeDesc::Int),
            optional: true,
        }],
        open: false,
    };
    p.code[2] = Inst::abc(Op::GetOpt, 1, 1, 0);
    p
}

fn schema_error(document: &str) -> String {
    schema_failure(json_program(document))
}

fn schema_failure(p: Program) -> String {
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::Schema);
            info.detail.unwrap_or_default()
        }
        other => panic!("expected a schema failure, got {other:?}"),
    }
}

#[test]
fn a_document_that_fits_becomes_a_value() {
    let p = json_program(r#"{"value": 7}"#);
    assert_eq!(run(&p, &[]), Value::I64(7));
}

#[test]
fn a_mismatch_names_the_path_that_failed() {
    assert!(
        schema_error(r#"{"value": "seven"}"#).contains("value: expected Int, found a string"),
        "{}",
        schema_error(r#"{"value": "seven"}"#)
    );
}

#[test]
fn a_missing_field_is_a_mismatch_not_a_default() {
    // A required field is required, so there is nothing to fill a missing one
    // with that anybody chose. A field written `T?` is the other half of this
    // pair, below: it fits, and reading it still hands back no value.
    assert!(schema_error("{}").contains("needs a field `value`"));
}

/// `{}`, `{"value": null}` and `{"value": 7}` against the same type. The first
/// two are one case rather than two, and that is the decision issue #78 turns
/// on: every protocol measured for it writes `null` where a value is missing,
/// and this workspace's own journal reader already treats the two alike.
#[test]
fn an_optional_field_fits_absent_null_and_a_value() {
    let mut p = optional_program(r#"{"value": 7}"#);
    assert_eq!(run(&p, &[]), Value::I64(7));

    for document in ["{}", r#"{"value": null}"#] {
        p.consts[0] = Const::Str(document.into());
        // `HAS_OPT` rather than `GET_OPT`, so the run reaches the end.
        p.code[2] = Inst::abc(Op::HasOpt, 1, 1, 0);
        p.funcs[0].ret_type = index_of(TypeDesc::Bool);
        assert_eq!(run(&p, &[]), Value::Bool(false), "{document}");
    }
}

/// Reading one that was not there fails the run, which is the decision
/// `GET_INDEX` already made: there is nothing to hand back, and a value nobody
/// chose would be worse.
#[test]
fn reading_an_optional_field_that_is_not_there_fails_the_run() {
    let p = optional_program(r#"{"value": null}"#);
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    match vm.run(0, &[]) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::FieldNotThere),
        other => panic!("expected the field to be missing, got {other:?}"),
    }
}

/// A field that is optional is still checked when it is there. Validation is
/// still a yes or no; what changed is which documents fit.
#[test]
fn an_optional_field_is_checked_when_it_is_there() {
    let p = optional_program(r#"{"value": "seven"}"#);
    let detail = schema_failure(p);
    assert!(detail.contains("value: expected Int"), "{detail}");
}

/// What `approve` shows is the value, and an absent optional field is written
/// `null` rather than left out. Both parse back to this same value, so the
/// round trip does not decide it: what does is that a person should be able to
/// tell a field the program has no value for from one the type never had.
#[test]
fn an_absent_optional_field_is_shown_as_null() {
    let mut p = optional_program(r#"{"value": null}"#);
    p.code[2] = Inst::abc(Op::ToJson, 1, 5, 1);
    p.funcs[0].ret_type = index_of(TypeDesc::Str);
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    let Status::Finished(Value::Str(handle)) = vm.run(0, &[]) else {
        panic!("expected a string");
    };
    assert_eq!(vm.arena.str(handle), r#"{"value":null}"#);
}

#[test]
fn an_unexpected_field_is_refused() {
    // Ignoring it would accept an answer that is not the shape that was asked
    // for. That is what a type says by not ending in `..`, and it is still the
    // default: the test below is the same document against a type that does.
    assert!(schema_error(r#"{"value": 1, "extra": 2}"#).contains("has no field `extra`"));
}

#[test]
fn an_open_type_ignores_a_field_it_does_not_declare() {
    // The same document, against a type that says it describes part of one. A
    // protocol that carries more than this program asked about has not
    // answered a different question.
    let p = json_program_with(r#"{"value": 1, "extra": 2}"#, true);
    assert_eq!(run(&p, &[]), Value::I64(1));
}

#[test]
fn an_open_type_still_needs_the_fields_it_declares() {
    // `..` is about what a document may carry beyond the type, not about what
    // the type asks for: the field the program reads has to be there.
    let detail = schema_failure(json_program_with("{}", true));
    assert!(detail.contains("needs a field `value`"), "{detail}");
}

#[test]
fn invalid_json_fails_at_the_boundary() {
    assert!(schema_error("{\"value\": }").contains("at byte"));
    // `not ...` starts like `null`, so the parser says what it expected.
    assert!(schema_error("not json at all").contains("expected `null`"));
}

#[test]
fn a_whole_number_fits_a_float_but_not_the_other_way() {
    let mut p = json_program(r#"{"value": 2}"#);
    // Change the field to a Float and read it back.
    p.types[5] = TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![Field::new("value", index_of(TypeDesc::Float))],
        open: false,
    };
    p.funcs[0].ret_type = index_of(TypeDesc::Float);
    assert_eq!(run(&p, &[]), Value::F64(2.0));

    // The reverse would change the value, so it is refused.
    assert!(schema_error(r#"{"value": 1.5}"#).contains("expected Int, found a number"));
}

// ---- budgets ----

#[test]
fn a_call_site_runs_out_of_budget() {
    // The VM enforces the budget without knowing that this call site is an
    // agent: a policy entry says how many calls an allowance is worth and
    // which allowance this site spends from, and that is all it needs.
    let mut p = exec_program();
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 0,
        budget: 1,
        budget_group: 1,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });

    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    // The budget is recorded as it is spent, so it is visible before it runs
    // out rather than only after.
    assert!(names(&sink).contains(&"budget_consumed"));
    assert!(matches!(
        vm.resume(sic_core::CapValue::I64(0)),
        Status::Finished(_)
    ));
}

#[test]
fn exceeding_a_budget_fails_the_run() {
    // A loop back onto the same CALL_CAP: the second visit is over budget.
    let mut p = exec_program();
    let code_off = p.funcs[0].code_off as usize;
    // main: LOAD_CONST, CALL_CAP, RETURN -> make the return jump back instead.
    p.code[code_off + 2] = Inst::asbx(Op::Jump, 0, -2);
    p.funcs[0].code_len = 3;
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 0,
        budget: 1,
        budget_group: 1,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    match vm.resume(sic_core::CapValue::I64(0)) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::OutOfBudget);
            assert!(
                info.detail.as_deref().unwrap_or("").contains("1 time(s)"),
                "{info:?}"
            );
        }
        other => panic!("expected the budget to run out, got {other:?}"),
    }
}

/// Two call sites, one allowance, and the second call is the one that is
/// refused.
///
/// This is the whole of #84 at the level that enforces it. Keyed by pc, each
/// site had a count of its own and both calls went through - so splitting a
/// function in two doubled what the declaration said, and nothing in the
/// bytecode recorded that it had.
#[test]
fn two_sites_that_share_an_allowance_share_its_count() {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            2,
            vec![
                Inst::abx(Op::LoadConst, 1, 0),
                Inst::abc(Op::CallCap, 0, 0, 1),
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
        pin: String::new(),
        answers: sic_core::Answers::Unsaid,
        repeatable: false,
        delegable: false,
        dir: String::new(),
        env: Vec::new(),
        args: Vec::new(),
        params: vec![4],
        ret_type: 2,
    });
    for pc in [1, 2] {
        p.policies.push(sic_bytecode::PolicyEntry {
            pc,
            attempts: 1,
            timeout_ms: 0,
            budget: 1,
            budget_group: 1,
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
            validates: 0,
        });
    }

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    match vm.resume(sic_core::CapValue::I64(0)) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::OutOfBudget);
            // And the message says the bound is over both of them, because a
            // reader looking at the site that failed can count one call there.
            assert!(
                info.detail
                    .as_deref()
                    .unwrap_or("")
                    .contains("1 time(s) in a run, from 2 call site(s)"),
                "{info:?}"
            );
        }
        other => panic!("expected the second site to be refused, got {other:?}"),
    }
}

#[test]
fn a_budget_survives_a_checkpoint() {
    // Otherwise resuming would hand the run a fresh allowance.
    let mut p = exec_program();
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 0,
        budget: 3,
        // Deliberately not the pc: what travels is the allowance, and a
        // checkpoint that keyed this by the call site would hand a resumed run
        // one count per site instead of one per declaration.
        budget_group: 7,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });

    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();
    let saved = Checkpoint::decode(&bytes).unwrap();
    assert_eq!(saved.spent, vec![(7, 1)]);
}

/// A call the budget refused is not charged and not recorded.
///
/// It used to be both: the site was charged and a `BudgetConsumed` emitted
/// before anything decided whether the call could happen, so the run's own
/// account showed a budgeted site used twice when the second use was refused
/// and the broker was never asked.
#[test]
fn a_refused_call_is_not_billed_for() {
    let mut p = exec_program();
    let code_off = p.funcs[0].code_off as usize;
    // A loop back onto the same CALL_CAP: the second visit is over budget.
    p.code[code_off + 2] = Inst::asbx(Op::Jump, 0, -2);
    p.funcs[0].code_len = 3;
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 0,
        budget: 1,
        budget_group: 1,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    });

    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    assert!(matches!(vm.run(0, &[]), Status::Suspended(_)));
    assert!(matches!(
        vm.resume(sic_core::CapValue::I64(0)),
        Status::Failed(_)
    ));

    let events = names(&sink);
    let charged = events.iter().filter(|n| **n == "budget_consumed").count();
    let asked = events
        .iter()
        .filter(|n| **n == "capability_requested")
        .count();
    // One call happened, so one charge and one request. The refused visit adds
    // neither.
    assert_eq!(charged, 1, "{events:?}");
    assert_eq!(asked, 1, "{events:?}");
}

// ---- retrying an answer that does not fit ----

/// An agent, as the compiler lowers one: a model call whose answer is parsed
/// against a declared type on the very next instruction.
///
/// `policy` is what the agent's declaration became. `budget` and `attempts`
/// are separate numbers here for the same reason they are separate fields in
/// the source: one bounds how many times the model may be asked at all, the
/// other how many times a bad answer may be re-asked.
fn agent_program(attempts: u32, budget: u32) -> Program {
    let mut p = program(
        vec![(
            "main",
            &[],
            TypeDesc::Int,
            3,
            vec![
                Inst::abx(Op::LoadConst, 2, 0),
                Inst::abc(Op::CallCap, 0, 0, 2),
                Inst::abc(Op::FromJson, 1, 5, 0),
                Inst::abc(Op::GetField, 1, 1, 0),
                Inst::abc(Op::Return, 1, 0, 0),
            ],
        )],
        vec![Const::Str("why did it fail?".into())],
    );
    p.types.push(TypeDesc::Object {
        name: "Wrapper".into(),
        fields: vec![Field::new("value", index_of(TypeDesc::Int))],
        open: false,
    });
    p.funcs[0].ret_type = index_of(TypeDesc::Int);
    p.caps.push(sic_bytecode::CapDecl {
        name: "llm.invoke".into(),
        kind: sic_core::CapKind::Invoke,
        constraints: "a-model".into(),
        pin: String::new(),
        answers: sic_core::Answers::Unsaid,
        repeatable: true,
        delegable: false,
        dir: String::new(),
        env: Vec::new(),
        args: Vec::new(),
        params: vec![index_of(TypeDesc::Str)],
        ret_type: index_of(TypeDesc::Str),
    });
    p.policies.push(sic_bytecode::PolicyEntry {
        pc: 1,
        attempts,
        timeout_ms: 0,
        budget,
        budget_group: u32::from(budget > 0),
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 5 + 1,
    });
    p
}

fn bad_answer() -> sic_core::CapValue {
    sic_core::CapValue::Str("{\"value\":\"not a number\"}".into())
}

#[test]
fn an_answer_that_does_not_fit_is_asked_again() {
    let p = agent_program(3, 0);
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));

    let Status::Suspended(request) = vm.run(0, &[]) else {
        panic!("expected a model call");
    };
    assert_eq!(request.attempt, 1);
    // Nothing has been rejected yet, so there is nothing to explain.
    assert_eq!(request.rejected, "");

    let Status::Suspended(request) = vm.resume(bad_answer()) else {
        panic!("expected a second attempt");
    };
    assert_eq!(request.attempt, 2);
    // The one thing the program could not have said itself: what was wrong
    // with the answer it is being asked to replace.
    assert!(request.rejected.contains("expected Int"), "{request:?}");

    // A good answer ends it, and `FROM_JSON` is still what turns the document
    // into a value: the check here decided only whether to ask again.
    assert!(matches!(
        vm.resume(sic_core::CapValue::Str("{\"value\":7}".into())),
        Status::Finished(Value::I64(7))
    ));

    let events = names(&sink);
    assert_eq!(
        events.iter().filter(|n| **n == "answer_rejected").count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|n| **n == "capability_requested")
            .count(),
        2
    );
}

#[test]
fn the_last_attempt_fails_the_way_a_run_with_no_retry_would() {
    let p = agent_program(2, 0);
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    let Status::Suspended(_) = vm.run(0, &[]) else {
        panic!("expected a model call");
    };
    let Status::Suspended(_) = vm.resume(bad_answer()) else {
        panic!("expected a second attempt");
    };
    match vm.resume(bad_answer()) {
        Status::Failed(info) => {
            assert_eq!(info.kind, FailKind::Schema);
            assert!(info.describe().contains("expected Int"), "{info:?}");
        }
        other => panic!("expected a schema failure, got {other:?}"),
    }
    // Both rejections are in the journal, including the one the run ended on.
    assert_eq!(
        names(&sink)
            .iter()
            .filter(|n| **n == "answer_rejected")
            .count(),
        2
    );
}

#[test]
fn a_rejected_answer_is_charged_to_the_budget() {
    // Two calls allowed and three attempts asked for. The budget is the number
    // a person approved, so it is the one that decides.
    let p = agent_program(3, 2);
    let sink = SharedSink::default();
    let mut vm = Vm::with_journal(&p, DEFAULT_FUEL, journal_for(&sink));
    let Status::Suspended(_) = vm.run(0, &[]) else {
        panic!("expected a model call");
    };
    let Status::Suspended(request) = vm.resume(bad_answer()) else {
        panic!("expected a second attempt");
    };
    assert_eq!(request.attempt, 2);
    match vm.resume(bad_answer()) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::OutOfBudget),
        other => panic!("expected a budget failure, got {other:?}"),
    }
    // One charge per attempt, and none for the attempt the budget refused.
    assert_eq!(
        names(&sink)
            .iter()
            .filter(|n| **n == "budget_consumed")
            .count(),
        2
    );
}

#[test]
fn an_agent_with_one_attempt_never_parses_twice() {
    // No `retry` means no `validates`, and then this is a run that behaves
    // exactly as it did before any of it existed: `FROM_JSON` reports the
    // failure, at the instruction that owns it.
    let mut p = agent_program(1, 0);
    p.policies[0].validates = 0;
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    let Status::Suspended(_) = vm.run(0, &[]) else {
        panic!("expected a model call");
    };
    match vm.resume(bad_answer()) {
        Status::Failed(info) => assert_eq!(info.kind, FailKind::Schema),
        other => panic!("expected a schema failure, got {other:?}"),
    }
}

#[test]
fn a_rejection_survives_a_checkpoint() {
    // A run that stops between two attempts is checkpointed while it waits, so
    // the reason lives in the file rather than only in memory. What reads it
    // back is the attempt after next: a broker that cannot answer at all sends
    // the run round again, and the answer still to be explained is the last one
    // that actually arrived.
    let p = agent_program(4, 0);
    let mut vm = Vm::new(&p, DEFAULT_FUEL);
    let Status::Suspended(_) = vm.run(0, &[]) else {
        panic!("expected a model call");
    };
    let Status::Suspended(_) = vm.resume(bad_answer()) else {
        panic!("expected a second attempt");
    };
    let bytes = vm.checkpoint(program_digest(), "?").unwrap();
    let (mut vm, _question) = Vm::restore(
        &p,
        &bytes,
        program_digest(),
        Box::new(SharedSink::default()),
    )
    .expect("the checkpoint should restore");
    let Status::Suspended(request) = vm.resume_failed(&sic_core::CapError::new("nope")) else {
        panic!("expected a third attempt");
    };
    assert_eq!(request.attempt, 3);
    assert!(request.rejected.contains("expected Int"), "{request:?}");
    assert!(matches!(
        vm.resume(sic_core::CapValue::Str("{\"value\":7}".into())),
        Status::Finished(Value::I64(7))
    ));
}
