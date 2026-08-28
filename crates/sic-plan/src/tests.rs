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
            answers: sic_core::Answers::Unsaid,
            repeatable: false,
            delegable: false,
            dir: String::new(),
            env: Vec::new(),
            args: Vec::new(),
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
        fields: vec![Field::new("cause", 4)],
        open: false,
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
            alternatives: None,
            remembers: false,
            tools: None,
            deadline_ms: None,
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
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
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
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
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
            type_name: "Diagnosis".into(),
            open: false,
        }
    );
}

/// A type that describes part of a document is a weaker claim than one that
/// describes all of it, and a plan that printed them the same way would say a
/// document was checked when only some of it was.
#[test]
fn an_open_type_says_so_in_the_plan() {
    let mut p = program_with_capability(None);
    p.code[2] = Inst::abc(Op::FromJson, 0, 5, 0);
    p.code.push(Inst::abc(Op::Return, 0, 0, 0));
    p.funcs[0].code_len = 4;
    p.types[5] = TypeDesc::Object {
        name: "Line".into(),
        fields: vec![Field::new("reason", 4)],
        open: true,
    };

    let plan = plan(&p, digest());
    assert_eq!(
        plan.functions[0].steps[1].action,
        Action::Verify {
            type_name: "Line".into(),
            open: true,
        }
    );
    assert!(render(&plan, "main.sic").contains("Line  (declared fields only)"));
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
        answers: sic_core::Answers::Unsaid,
        repeatable: false,
        delegable: false,
        dir: String::new(),
        env: Vec::new(),
        args: Vec::new(),
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
    // The column widths are formatting; what matters is that the verb and the
    // capability are both on the line.
    assert!(text.contains("READ"), "{text}");
    assert!(text.contains("fs.read"), "{text}");
}

/// A routed grant is reported as routed, and a translated one as the agent's
/// own rule. The difference is the whole of §3 and §4 of the authority design,
/// and a plan that blurred it would be describing a boundary it does not have.
#[test]
fn the_plan_tells_a_translated_grant_from_a_routed_one() {
    let manifest = [
        ("llm.invoke", CapKind::Invoke, "claude"),
        ("fs.read", CapKind::Read, "./docs"),
        ("process.exec", CapKind::Exec, "/usr/bin/cargo"),
    ];
    let grants: Vec<Grant> = manifest
        .iter()
        .map(|(name, kind, constraint)| Grant {
            name: (*name).to_string(),
            kind: *kind,
            constraint: (*constraint).to_string(),
            pin: String::new(),
            args: Vec::new(),
            repeatable: false,
            // The `process` grant is delegated here, because what this test is
            // about is the difference between a translated grant and a routed
            // one. The difference a withheld grant makes is its own test.
            delegable: *name == "process.exec",
            dir: String::new(),
            env: Vec::new(),
            answers: sic_core::Answers::Unsaid,
            called_from: Vec::new(),
        })
        .collect();

    let text = super::agent_authority(&grants);
    assert!(text.contains("the agent's Read   \"./docs\""), "{text}");
    assert!(text.contains("(its own permissions)"), "{text}");
    assert!(
        text.contains("the agent may use  \"/usr/bin/cargo\""),
        "{text}"
    );
    assert!(text.contains("(through the broker)"), "{text}");
    // Every line says where, and the three that are true of every agent are
    // always there.
    for rendered in text.lines() {
        assert!(rendered.ends_with(')'), "{rendered}");
    }
}

/// The same manifest without the word: the program keeps `cargo` and the agent
/// is told, on its own line, that it does not get it. Saying so is the point -
/// a reader deciding whether to run this needs to know the two are different.
#[test]
fn a_withheld_grant_is_a_line_rather_than_an_absence() {
    let grants = vec![
        Grant {
            name: "llm.invoke".to_string(),
            kind: CapKind::Invoke,
            constraint: "claude".to_string(),
            pin: String::new(),
            args: Vec::new(),
            repeatable: false,
            delegable: false,
            dir: String::new(),
            env: Vec::new(),
            answers: sic_core::Answers::Unsaid,
            called_from: Vec::new(),
        },
        Grant {
            name: "process.exec".to_string(),
            kind: CapKind::Exec,
            constraint: "/usr/bin/cargo".to_string(),
            pin: String::new(),
            args: Vec::new(),
            repeatable: false,
            delegable: false,
            dir: String::new(),
            env: Vec::new(),
            answers: sic_core::Answers::Unsaid,
            called_from: Vec::new(),
        },
    ];
    let text = super::agent_authority(&grants);
    assert!(
        text.contains("the agent may not  use \"/usr/bin/cargo\""),
        "{text}"
    );
    assert!(text.contains("does not say `delegable`"), "{text}");
    assert!(!text.contains("the agent may use"), "{text}");
    for rendered in text.lines() {
        assert!(rendered.ends_with(')'), "{rendered}");
    }
}
