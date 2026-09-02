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
            budget_sites: 0,
            attempts: 1,
            until_it_fits: false,
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
fn only_a_budget_bounds_a_call_over_a_run() {
    // `retry` says how many times one visit may call out; how many visits
    // there are depends on the path taken and on recursion.
    let unbudgeted = plan(
        &program_with_capability(Some(PolicyEntry {
            pc: 1,
            attempts: 5,
            timeout_ms: 250,
            budget: 0,
            budget_group: 0,
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
            validates: 0,
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
            budget_group: 1,
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
            validates: 0,
        })),
        digest(),
    );
    assert_eq!(budgeted.bounded_calls, 2);
    assert_eq!(budgeted.unbounded_sites, 0);
    let text = render(&budgeted, "main.sic");
    assert!(text.contains("At most 2 capability call(s)"), "{text}");
    // One site, so the number on its line is its own and the line says
    // nothing about sharing. The allowance is still named once, under
    // `Budgets`.
    assert!(
        text.contains("at most 2 in a run  5 attempts each"),
        "{text}"
    );
    assert!(!text.contains("shared by"), "{text}");

    assert!(
        text.contains("at most 2 fs.read calls in a run, from 1 site: main 3:12"),
        "{text}"
    );

    // An agent's retry is about the shape of the answer, and every attempt at
    // one comes out of the allowance on the same line. So the line says so, in
    // different words, and does not invite a reader to multiply the two.
    let validated = plan(
        &program_with_capability(Some(PolicyEntry {
            pc: 1,
            attempts: 3,
            timeout_ms: 0,
            budget: 3,
            budget_group: 1,
            conversation: 0,
            tools: 0,
            deadline_ms: 0,
            validates: 1,
        })),
        digest(),
    );
    assert_eq!(validated.bounded_calls, 3);
    let text = render(&validated, "main.sic");
    assert!(
        text.contains("at most 3 in a run  at most 3 attempts at an answer that fits"),
        "{text}"
    );
    assert!(!text.contains("attempts each"), "{text}");
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

// ---- the plan as data ----

/// Reads one member out of a flat object. Enough for a test, and deliberately
/// not a JSON reader: `sic-json` is the parser, and this file is checking what
/// was written rather than re-deriving it.
fn member<'a>(json: &'a str, name: &str) -> &'a str {
    let at = json
        .find(&format!("\"{name}\":"))
        .unwrap_or_else(|| panic!("no `{name}` in {json}"));
    let rest = &json[at + name.len() + 3..];
    let end = rest
        .find([',', '}', ']'])
        .unwrap_or_else(|| panic!("unterminated `{name}` in {json}"));
    rest[..end].trim_matches('"')
}

#[test]
fn the_plan_as_data_says_what_the_prose_says() {
    let p = program_with_capability(Some(PolicyEntry {
        pc: 1,
        attempts: 3,
        timeout_ms: 250,
        budget: 2,
        budget_group: 1,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    }));
    let plan = plan(&p, digest());
    let json = to_json(&plan);

    // The version is first, so a reader that does not recognise it can stop
    // before it has assumed anything.
    assert!(json.starts_with("{\"version\":1,"), "{json}");
    // The digest is what ties an approved plan to a run, and it is the same
    // digest the prose prints.
    assert_eq!(member(&json, "bytecode"), digest().to_string());
    assert!(render(&plan, "main.sic").contains(&digest().to_string()));

    // The numbers a rule would be written against.
    assert_eq!(member(&json, "capability"), "fs.read");
    assert_eq!(member(&json, "effect"), "read");
    assert_eq!(member(&json, "constraint"), "./a.txt");
    assert_eq!(member(&json, "budget"), "2");
    assert_eq!(member(&json, "attempts"), "3");
    assert_eq!(member(&json, "timeout_ms"), "250");
    assert_eq!(member(&json, "bounded_calls"), "2");
}

/// An absence is `null` rather than a zero or a missing member, because a rule
/// that could not tell "no budget" from "a budget of nothing" would be a rule
/// answering a different question than the one it was written to ask.
#[test]
fn what_a_plan_does_not_know_is_null_rather_than_zero() {
    let plan = plan(&program_with_capability(None), digest());
    let json = to_json(&plan);
    assert_eq!(member(&json, "budget"), "null");
    assert_eq!(member(&json, "timeout_ms"), "null");
    assert_eq!(member(&json, "tools"), "null");
    assert_eq!(member(&json, "deadline_ms"), "null");
    assert_eq!(member(&json, "pin"), "null");
    // And a site with no bound is counted as a site rather than as a number of
    // calls, which is the plan refusing to invent one.
    assert_eq!(member(&json, "bounded_calls"), "0");
    assert_eq!(member(&json, "unbounded_sites"), "1");
}

/// Both renderings come from one `Plan`, and this is what holds them to it.
///
/// The failure this guards against is not a formatting difference. It is a rule
/// passing against a plan a person reading the prose would have refused - which
/// is what a second walk of the bytecode would eventually produce, and why
/// there is not one.
#[test]
fn every_step_the_prose_numbers_is_a_step_in_the_data() {
    let p = program_with_capability(Some(PolicyEntry {
        pc: 1,
        attempts: 1,
        timeout_ms: 0,
        budget: 0,
        budget_group: 0,
        conversation: 0,
        tools: 0,
        deadline_ms: 0,
        validates: 0,
    }));
    let plan = plan(&p, digest());
    let prose = render(&plan, "main.sic");
    let json = to_json(&plan);

    let numbered = prose
        .lines()
        .filter(|l| l.trim_start().starts_with("1. ") || l.trim_start().starts_with("2. "))
        .count();
    let in_data = json.matches("\"pc\":").count();
    assert_eq!(numbered, in_data, "prose:\n{prose}\ndata:\n{json}");
    assert!(numbered > 0);

    // And the verb a person reads is the verb a rule reads.
    assert!(prose.contains("READ"), "{prose}");
    assert_eq!(member(&json, "verb"), "READ");
}

/// A constraint is a string from the source and can hold anything. The prose
/// has the same problem and solves it for Mermaid; this is the other renderer's
/// version, and the answer is that the escaping is `sic-json`'s rather than
/// this file's.
#[test]
fn a_constraint_that_would_break_the_shape_is_escaped() {
    let mut p = program_with_capability(None);
    p.caps[0].constraints = "a \"quoted\" \\ path\nwith a newline".into();
    let json = to_json(&plan(&p, digest()));
    assert!(
        json.contains(r#""a \"quoted\" \\ path\nwith a newline""#),
        "{json}"
    );
}

// ---- where a model's answer goes ----

/// A model call, a laundering step, and a write - the three instructions the
/// question is about, with the middle one chosen by the caller.
///
/// `launder` is `Op::Approve` for a program this compiler produced, and
/// `Op::Move` for what one with three lines removed would produce. The two are
/// the same copy and the same run; the difference is only whether the file says
/// a person agreed, which is the whole of what this analysis reads.
fn program_with_a_model_and_a_write(launder: Op) -> Program {
    program_with_a_source_and_a_write(launder, "llm.invoke", CapKind::Invoke)
}

/// The same, with the capability whose answer is carried named by the caller.
fn program_with_a_source_and_a_write(launder: Op, source: &str, kind: CapKind) -> Program {
    Program {
        consts: vec![Const::Str("./out.txt".into())],
        types: TypeDesc::primitives(),
        funcs: vec![FuncDef {
            name: "main".into(),
            params: Vec::new(),
            reg_count: 6,
            ret_type: 4,
            code_off: 0,
            code_len: 7,
        }],
        caps: vec![
            CapDecl {
                name: source.into(),
                kind,
                constraints: "a-model".into(),
                pin: String::new(),
                answers: sic_core::Answers::Unsaid,
                repeatable: false,
                delegable: false,
                dir: String::new(),
                env: Vec::new(),
                args: Vec::new(),
                params: vec![4],
                ret_type: 4,
            },
            CapDecl {
                name: "fs.write".into(),
                kind: CapKind::Write,
                constraints: "./out.txt".into(),
                pin: String::new(),
                answers: sic_core::Answers::Unsaid,
                repeatable: false,
                delegable: false,
                dir: String::new(),
                env: Vec::new(),
                args: vec![],
                params: vec![4, 4],
                ret_type: 2,
            },
        ],
        code: vec![
            Inst::abx(Op::LoadConst, 1, 0),  // r1 = "./out.txt"
            Inst::abc(Op::CallCap, 2, 0, 1), // r2 = llm.invoke(r1)
            Inst::abc(launder, 3, 2, 0),     // r3 = <launder> r2
            Inst::abc(Op::Move, 4, 1, 0),    // r4 = r1   (the path)
            Inst::abc(Op::Move, 5, 3, 0),    // r5 = r3   (what to write)
            Inst::abc(Op::CallCap, 0, 1, 4), // fs.write(r4, r5)
            Inst::abc(Op::Return, 0, 0, 0),
        ],
        policies: Vec::new(),
        debug: DebugInfo {
            sources: vec!["main.sic".into()],
            lines: vec![(5, 0, 9, 5)],
        },
    }
}

/// The question a person approving a plan actually has, and the one the
/// manifest cannot answer: not what may be reached, but what may be carried
/// there.
#[test]
fn a_plan_says_where_a_models_answer_goes() {
    let plan = plan(&program_with_a_model_and_a_write(Op::Approve), digest());
    assert_eq!(plan.flows.len(), 1, "{:?}", plan.flows);
    let flow = &plan.flows[0];
    assert_eq!(flow.cap, "fs.write");
    assert_eq!(flow.func, "main");
    assert_eq!(flow.position, Some((9, 5)));
    assert!(flow.approved);

    let text = render(&plan, "main.sic");
    assert!(
        text.contains("Nobody signed off on what reaches:"),
        "{text}"
    );
    assert!(text.contains("fs.write in main at 9:5"), "{text}");
    assert!(text.contains("(a person agreed)"), "{text}");
}

/// The case the whole issue is about: bytecode this compiler would not have
/// produced.
///
/// `APPROVE` and `MOVE` are the same copy and the same run. E0372 refuses the
/// source that would need the second, but E0372 is the type checker's and the
/// artifact everything downstream trusts is the file. With the fact in the
/// file, a reader of the file can see it is missing.
#[test]
fn a_flow_nobody_agreed_to_is_the_one_that_is_marked() {
    let plan = plan(&program_with_a_model_and_a_write(Op::Move), digest());
    assert_eq!(plan.flows.len(), 1, "{:?}", plan.flows);
    assert!(!plan.flows[0].approved);

    // The weaker claim is the one marked, the other way round from the rest of
    // this plan: a reader scanning the list must not have to notice a missing
    // word.
    let text = render(&plan, "main.sic");
    assert!(text.contains("** nobody was asked **"), "{text}");
    assert!(!text.contains("(a person agreed)"), "{text}");

    // And a rule can be written against it, which is what makes this a
    // property somebody can check rather than a sentence somebody can read.
    let json = to_json(&plan);
    assert!(json.contains("\"capability\":\"fs.write\""), "{json}");
    assert!(json.contains("\"approved\":false"), "{json}");
}

/// A model's answer that goes nowhere is not a flow, and a plan that said
/// otherwise would train its reader to skip the section.
#[test]
fn a_model_call_on_its_own_is_not_a_flow() {
    let mut p = program_with_a_model_and_a_write(Op::Approve);
    // Write the path twice instead of the answer.
    p.code[4] = Inst::abc(Op::Move, 5, 1, 0);
    assert!(plan(&p, digest()).flows.is_empty());
    assert!(!render(&plan(&p, digest()), "main.sic").contains("Nobody signed off on what reaches"));
}

/// The compiler reuses one register window for every call's arguments, so a
/// register that held a prompt later holds a path.
///
/// This is why the analysis is flow-sensitive rather than a taint per register
/// per function. Without it every program that calls a model would be reported
/// as carrying its answer into everything it does afterwards - and a section
/// that fires on every program is one nobody reads.
#[test]
fn a_register_reused_after_a_model_call_is_not_still_the_model() {
    let mut p = program_with_a_model_and_a_write(Op::Approve);
    // r2 held the model's answer; now it holds the path, and the write is
    // given that.
    p.code[2] = Inst::abc(Op::Move, 2, 1, 0);
    p.code[4] = Inst::abc(Op::Move, 5, 2, 0);
    assert!(
        plan(&p, digest()).flows.is_empty(),
        "{:?}",
        plan(&p, digest()).flows
    );
}

/// Reading is not changing, so a model's answer reaching `fs.read` is not this.
#[test]
fn only_a_capability_that_changes_something_is_a_sink() {
    let mut p = program_with_a_model_and_a_write(Op::Move);
    p.caps[1].kind = CapKind::Read;
    p.caps[1].name = "fs.read".into();
    assert!(plan(&p, digest()).flows.is_empty());
}

/// Every capability whose answer the type checker labels is a source here.
///
/// #93: the analysis keyed on one name and four others answer with values
/// nobody signed off. The one that mattered most is `process.run`, because it
/// is the most used and because its `Exit` is not itself labelled - the label
/// is on a field, and a reader that stopped at the outside would have agreed
/// with the old list and disagreed with the compiler.
#[test]
fn a_program_that_read_a_log_is_a_source_too() {
    for (name, kind) in [
        ("llm.invoke", CapKind::Invoke),
        ("process.capture", CapKind::Exec),
        ("process.run", CapKind::Exec),
        ("git.status", CapKind::Read),
        ("git.rev_parse", CapKind::Read),
    ] {
        let p = program_with_a_source_and_a_write(Op::Move, name, kind);
        let flows = plan(&p, digest()).flows;
        assert_eq!(flows.len(), 1, "{name} was not seen: {flows:?}");
        assert!(!flows[0].approved, "{name}");
        assert_eq!(flows[0].cap, "fs.write");
    }
}

/// And asking a person which of the program's own alternatives is not one.
/// The text was written by whoever wrote the program, which is the whole of
/// why `choose` carries no restriction.
#[test]
fn asking_a_person_to_choose_is_not_a_source() {
    let p = program_with_a_source_and_a_write(Op::Move, "human.choose", CapKind::Invoke);
    assert!(plan(&p, digest()).flows.is_empty());
}
