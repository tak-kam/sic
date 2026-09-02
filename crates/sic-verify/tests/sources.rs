//! The list of capabilities whose answers nobody signed off is a second copy.
//!
//! `sic-plan` reads bytecode, where trust is erased, so it cannot work out
//! which capabilities answer with a labelled value - it has to be told. Being
//! told is fine; being told once and never again is not. The next capability
//! somebody adds with an `Observed<T>` return would otherwise be a flow the
//! plan does not mention, and a plan that misses a flow is a false assurance
//! about the one question this language exists to answer.
//!
//! `sic-types` is a dev-dependency for this, and must not become a real one.

use sic_core::TypeId;
use sic_types::cap::BUILTIN_CAPS;
use sic_types::ty::Types;

/// Whether a type has a trust label anywhere inside it.
///
/// Into fields as well, which is the case that matters: `process.run` answers
/// with an `Exit`, and `Exit` is not labelled - its `output` field is. A walk
/// that stopped at the outside would agree with `flow.rs` today and disagree
/// with the checker, which is the drift this test exists to catch.
fn carries_a_label(types: &Types, id: TypeId, depth: usize) -> bool {
    // A type can reach itself through an optional field, so the walk is bounded
    // rather than trusting the shape of the type graph.
    if depth > 16 {
        return false;
    }
    if types.trust_of(id).is_some() {
        return true;
    }
    if let Some(element) = types.list_element(id) {
        return carries_a_label(types, element, depth + 1);
    }
    if let Some(object) = types.as_object(id) {
        return types
            .object(object)
            .fields
            .iter()
            .any(|f| carries_a_label(types, f.ty, depth + 1));
    }
    false
}

/// The same predicate `crates/sic-plan/src/flow.rs` holds, spelled out here so
/// that a change to one is a failure rather than a silence.
fn the_plan_calls_it_untrusted(cap: &str) -> bool {
    matches!(
        cap,
        "llm.invoke" | "process.capture" | "process.run" | "git.status" | "git.rev_parse"
    )
}

#[test]
fn the_plan_agrees_with_the_checker_about_which_answers_nobody_signed_off() {
    let types = Types::new();
    let mut wrong = Vec::new();
    for sig in BUILTIN_CAPS {
        let labelled = carries_a_label(&types, sig.ret, 0);
        let listed = the_plan_calls_it_untrusted(sig.name);
        if labelled != listed {
            wrong.push(format!(
                "{}: the checker says {}, `flow.rs` says {}",
                sig.name,
                match labelled {
                    true => "nobody signed off on its answer",
                    false => "its answer is the program's own",
                },
                match listed {
                    true => "untrusted",
                    false => "trusted",
                }
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the list in `crates/sic-plan/src/flow.rs` has drifted from `cap.rs`. \
         Being wrong the second way - the checker labels it and the plan does \
         not - is a flow the plan will not mention:\n{}",
        wrong.join("\n")
    );
}

/// And the list is not vacuously right: there is something in it, and something
/// out of it.
#[test]
fn the_two_sides_are_both_populated() {
    let types = Types::new();
    let labelled: Vec<&str> = BUILTIN_CAPS
        .iter()
        .filter(|s| carries_a_label(&types, s.ret, 0))
        .map(|s| s.name)
        .collect();
    assert!(labelled.contains(&"llm.invoke"), "{labelled:?}");
    // The four #93 was about, and the reason it was filed.
    assert!(labelled.contains(&"process.capture"), "{labelled:?}");
    assert!(labelled.contains(&"process.run"), "{labelled:?}");
    assert!(labelled.contains(&"git.status"), "{labelled:?}");
    assert!(labelled.contains(&"git.rev_parse"), "{labelled:?}");
    // Asking a person which of the program's own alternatives is not this:
    // the text was written by whoever wrote the program.
    assert!(!labelled.contains(&"human.choose"), "{labelled:?}");
    assert!(!labelled.contains(&"human.approve"), "{labelled:?}");
    assert!(!labelled.contains(&"fs.read"), "{labelled:?}");
}
