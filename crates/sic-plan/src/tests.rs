use sic_bytecode::inst::{Inst, Op};
use sic_bytecode::program::*;
use sic_core::{CapKind, Digest};

use super::*;

fn digest() -> Digest {
    Digest::of(b"a program")
}

/// A module with one capability and a call site for it.
fn program_with_capability(policy: Option<PolicyEntry>) -> Program {
    let mut p = Program {
        consts: vec![Const::Str("./a.txt".into())],
        types: TypeDesc::primitives(),
        funcs: vec![FuncDef {
            name: "main".into(),
            params: Vec::new(),
            reg_count: 2,
            ret_type: 4,
            code_off: 0,
            code_len: 3,
        }],
        caps: vec![CapDecl {
            name: "fs.read".into(),
            kind: CapKind::Read,
            constraints: "./a.txt".into(),
            pin: String::new(),
            params: vec![4],
            ret_type: 4,
        }],
        code: vec![
            Inst::abx(Op::LoadConst, 1, 0),
            Inst::abc(Op::CallCap, 0, 0, 1),
            Inst::abc(Op::Return, 0, 0, 0),
        ],
        policies: policy.into_iter().collect(),
        debug: DebugInfo {
            sources: vec!["main.sic".into()],
            lines: vec![(1, 0, 3, 12)],
        },
    };
    p.types.push(TypeDesc::Object {
        name: "Diagnosis".into(),
        fields: vec![("cause".into(), 4)],
    });
    p
}

#[test]
fn a_capability_call_is_listed_with_what_bounds_it() {
    let p = program_with_capability(None);
    let plan = plan(&p, digest());

    assert_eq!(plan.functions.len(), 1);
    let step = &plan.functions[0].steps[0];
    assert_eq!(step.action.verb(), "READ");
    assert_eq!(
        step.action,
        Action::Capability {
            name: "fs.read".into(),
            kind: CapKind::Read,
            constraint: "./a.txt".into(),
            budget: None,
            attempts: 1,
            timeout_ms: 0,
        }
    );
    // The debug section puts it back on a line of source.
    assert_eq!(step.position, Some((3, 12)));
    // No budget, so there is no number to give: how often it runs depends on
    // the path taken.
    assert_eq!(plan.bounded_calls, 0);
    assert_eq!(plan.unbounded_sites, 1);
}

#[test]
fn only_a_budget_bounds_a_site_over_a_run() {
    // `retry` says how many times one visit may call out; how many visits
    // there are depends on the path taken and on recursion.
    let unbudgeted = plan(
        &program_with_capability(Some(PolicyEntry {
            pc: 1,
            attempts: 5,
            timeout_ms: 250,
            budget: 0,
        })),
        digest(),
    );
    assert_eq!(unbudgeted.bounded_calls, 0);
    assert_eq!(unbudgeted.unbounded_sites, 1);
    let text = render(&unbudgeted, "main.sic");
    assert!(text.contains("none with a budget"), "{text}");
    assert!(text.contains("5 attempts each"), "{text}");

    // A budget is a real bound.
    let budgeted = plan(
        &program_with_capability(Some(PolicyEntry {
            pc: 1,
            attempts: 5,
            timeout_ms: 0,
            budget: 2,
        })),
        digest(),
    );
    assert_eq!(budgeted.bounded_calls, 2);
    assert_eq!(budgeted.unbounded_sites, 0);
    assert!(render(&budgeted, "main.sic").contains("At most 2 capability call(s)"));
}

#[test]
fn the_kind_comes_from_the_manifest_not_a_guess() {
    // "Does this only look, or does it change something" is the manifest's
    // answer.
    for (kind, verb) in [
        (CapKind::Read, "READ"),
        (CapKind::Write, "WRITE"),
        (CapKind::Exec, "EXEC"),
        (CapKind::Invoke, "INVOKE"),
    ] {
        let mut p = program_with_capability(None);
        p.caps[0].kind = kind;
        assert_eq!(plan(&p, digest()).functions[0].steps[0].action.verb(), verb);
    }
}

#[test]
fn a_validation_is_a_step() {
    // An agent shows as its two steps, because that is what it is.
    let mut p = program_with_capability(None);
    p.code[2] = Inst::abc(Op::FromJson, 0, 5, 0);
    p.code.push(Inst::abc(Op::Return, 0, 0, 0));
    p.funcs[0].code_len = 4;

    let plan = plan(&p, digest());
    let verbs: Vec<&str> = plan.functions[0]
        .steps
        .iter()
        .map(|s| s.action.verb())
        .collect();
    assert_eq!(verbs, vec!["READ", "VERIFY"]);
    assert_eq!(
        plan.functions[0].steps[1].action,
        Action::Verify {
            type_name: "Diagnosis".into()
        }
    );
}

#[test]
fn tasks_appear_as_steps() {
    let mut p = program_with_capability(None);
    p.code[1] = Inst::abc(Op::Spawn, 0, 0, 1);
    p.code[2] = Inst::abc(Op::Await, 0, 0, 0);
    p.code.push(Inst::abc(Op::Return, 0, 0, 0));
    p.funcs[0].code_len = 4;

    let plan = plan(&p, digest());
    let verbs: Vec<&str> = plan.functions[0]
        .steps
        .iter()
        .map(|s| s.action.verb())
        .collect();
    assert_eq!(verbs, vec!["SPAWN", "AWAIT"]);
    // Spawning is not itself an effect.
    assert_eq!(plan.bounded_calls, 0);
    assert_eq!(plan.unbounded_sites, 0);
}

#[test]
fn a_granted_but_uncalled_capability_is_named() {
    let mut p = program_with_capability(None);
    p.caps.push(CapDecl {
        name: "process.exec".into(),
        kind: CapKind::Exec,
        constraints: "/usr/bin/true".into(),
        pin: String::new(),
        params: vec![4],
        ret_type: 2,
    });
    let plan = plan(&p, digest());
    assert_eq!(plan.unused, vec!["process.exec".to_string()]);
    // It is still in the manifest, because the grant exists either way.
    assert_eq!(plan.capabilities.len(), 2);
}

#[test]
fn a_plan_says_whether_a_grant_pins_what_runs() {
    // It is what a reader most wants to know about a grant.
    let unpinned = plan(&program_with_capability(None), digest());
    assert!(render(&unpinned, "main.sic").contains("(not pinned)"));

    let mut p = program_with_capability(None);
    p.caps[0].pin = "b".repeat(64);
    let text = render(&plan(&p, digest()), "main.sic");
    assert!(
        text.contains(&format!("sha256:{}", "b".repeat(64))),
        "{text}"
    );
}

#[test]
fn a_program_with_no_effects_says_so() {
    let p = Program {
        types: TypeDesc::primitives(),
        funcs: vec![FuncDef {
            name: "main".into(),
            params: Vec::new(),
            reg_count: 1,
            ret_type: 2,
            code_off: 0,
            code_len: 1,
        }],
        code: vec![Inst::abc(Op::Return, 0, 0, 0)],
        ..Program::default()
    };
    let plan = plan(&p, digest());
    assert!(plan.functions.is_empty());
    assert_eq!(plan.bounded_calls, 0);

    let text = render(&plan, "main.sic");
    assert!(text.contains("(no external effects)"), "{text}");
    assert!(text.contains("(none)"), "{text}");
    assert!(text.contains("No capability calls."), "{text}");
}

#[test]
fn the_rendering_names_the_bytecode_it_is_of() {
    // Approving a plan should be tied to running that exact program.
    let plan = plan(&program_with_capability(None), digest());
    let text = render(&plan, "main.sic");
    assert!(text.contains(&digest().to_string()), "{text}");
    assert!(text.contains("READ   fs.read"), "{text}");
}
