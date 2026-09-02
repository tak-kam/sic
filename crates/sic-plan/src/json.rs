//! The plan, as data.
//!
//! A plan exists so that a program can be approved before it is run, and until
//! now the only thing that could perform that approval was a person reading
//! prose. Three things that should be able to read a plan could not: a rule
//! about a repository ("nothing writes outside `./out`"), a diff of what a
//! branch may now do that it could not before, and anything that wants to sort
//! or filter a plan of forty sites. `diff` over rendered text answers the
//! second badly - a column shifts when a constraint gets longer, and a widened
//! grant hides in a line that looks unchanged.
//!
//! **This is not a second walk of the bytecode.** It renders the same `Plan`
//! that `render` renders, so the two cannot disagree about what a program may
//! do; the failure mode that would be is a rule passing against a plan a person
//! would have refused. `crates/sic-plan/src/tests.rs` holds them to it.
//!
//! Written by hand, like every other serialiser here, and it is the shape that
//! needs the argument rather than the code: something will be checking a field
//! name against a rule somebody wrote, so this is an interface. `version` is
//! the first field for that reason - a reader that does not recognise it should
//! stop rather than guess.

use sic_json::write_quoted;

use crate::{Action, BudgetPlan, BudgetSite, FunctionPlan, Grant, Plan, Reaches, Step};

/// The shape's own version, which moves when a reader that assumed the old one
/// would be wrong rather than merely incomplete.
///
/// Adding a field does not move it: a reader that ignores what it does not know
/// is still right about everything else. Removing one, renaming one, or
/// changing what a value means does, because each of those makes a rule quietly
/// answer a different question than the one it was written to ask.
pub const VERSION: u32 = 1;

/// The plan as a single JSON object.
pub fn to_json(plan: &Plan) -> String {
    let mut out = String::new();
    out.push('{');
    field_u64(&mut out, "version", VERSION as u64, true);
    field_str(&mut out, "bytecode", &plan.digest.to_string(), false);
    field_bool(&mut out, "multi_file", plan.multi_file, false);

    out.push_str(",\"functions\":");
    list(&mut out, &plan.functions, function);

    out.push_str(",\"capabilities\":");
    list(&mut out, &plan.capabilities, grant);

    out.push_str(",\"budgets\":");
    list(&mut out, &plan.budgets, budget);

    out.push_str(",\"flows\":");
    list(&mut out, &plan.flows, flow);

    out.push_str(",\"reaches\":");
    list(&mut out, &plan.reaches, reaches);

    out.push_str(",\"unused\":");
    list(&mut out, &plan.unused, |out, name| write_quoted(out, name));

    // The two halves of "how much may this program do", and they are not one
    // number: a budget bounds an allowance over a whole run, and a site with no
    // budget runs as often as the path taken says. Reporting the second as a
    // count of sites rather than a count of calls is the plan refusing to
    // invent one.
    field_u64(&mut out, "bounded_calls", plan.bounded_calls, false);
    field_u64(
        &mut out,
        "unbounded_sites",
        plan.unbounded_sites as u64,
        false,
    );
    out.push('}');
    out
}

fn function(out: &mut String, f: &FunctionPlan) {
    out.push('{');
    field_str(out, "name", &f.name, true);
    out.push_str(",\"steps\":");
    list(out, &f.steps, step);
    out.push('}');
}

fn step(out: &mut String, s: &Step) {
    out.push('{');
    field_u64(out, "pc", s.pc as u64, true);
    field_str(out, "verb", s.action.verb(), false);
    position(out, s.position, &s.file);
    out.push_str(",\"action\":");
    action(out, &s.action);
    out.push('}');
}

fn action(out: &mut String, a: &Action) {
    out.push('{');
    match a {
        Action::Capability {
            name,
            kind,
            constraint,
            budget,
            budget_sites,
            attempts,
            until_it_fits,
            timeout_ms,
            alternatives,
            remembers,
            tools,
            deadline_ms,
        } => {
            field_str(out, "kind", "capability", true);
            field_str(out, "capability", name, false);
            field_str(out, "effect", kind_name(*kind), false);
            field_str(out, "constraint", constraint, false);
            // `null` where the prose prints nothing, so that a rule can tell
            // "no budget" from "a budget of zero" without knowing that the
            // second cannot happen.
            optional_u64(out, "budget", budget.map(u64::from));
            field_u64(out, "budget_sites", *budget_sites as u64, false);
            field_u64(out, "attempts", *attempts as u64, false);
            field_bool(out, "attempts_are_about_the_shape", *until_it_fits, false);
            optional_u64(out, "timeout_ms", nonzero(*timeout_ms));
            optional_u64(out, "alternatives", alternatives.map(u64::from));
            field_bool(out, "remembers", *remembers, false);
            optional_u64(out, "tools", tools.map(u64::from));
            optional_u64(out, "deadline_ms", deadline_ms.map(u64::from));
        }
        Action::Verify { type_name, open } => {
            field_str(out, "kind", "verify", true);
            field_str(out, "type", type_name, false);
            field_bool(out, "declared_fields_only", *open, false);
        }
        Action::Spawn { func } => {
            field_str(out, "kind", "spawn", true);
            field_str(out, "function", func, false);
        }
        Action::Await => field_str(out, "kind", "await", true),
    }
    out.push('}');
}

fn grant(out: &mut String, g: &Grant) {
    out.push('{');
    field_str(out, "capability", &g.name, true);
    field_str(out, "effect", kind_name(g.kind), false);
    field_str(out, "constraint", &g.constraint, false);
    // Empty means unpinned, and `null` says so rather than leaving a reader to
    // decide what an empty digest is.
    optional_str(out, "pin", (!g.pin.is_empty()).then_some(g.pin.as_str()));
    out.push_str(",\"args\":");
    list(out, &g.args, |out, a| write_quoted(out, a));
    field_bool(out, "repeatable", g.repeatable, false);
    field_bool(out, "delegable", g.delegable, false);
    optional_str(out, "dir", (!g.dir.is_empty()).then_some(g.dir.as_str()));
    out.push_str(",\"env\":");
    list(out, &g.env, |out, (k, v)| {
        out.push('{');
        field_str(out, "name", k, true);
        field_str(out, "value", v, false);
        out.push('}');
    });
    field_str(out, "answers", answers_name(g.answers), false);
    out.push_str(",\"called_from\":");
    list(out, &g.called_from, |out, f| write_quoted(out, f));
    out.push('}');
}

fn budget(out: &mut String, b: &BudgetPlan) {
    out.push('{');
    field_u64(out, "group", b.group as u64, true);
    field_str(out, "capability", &b.cap, false);
    field_u64(out, "calls", b.calls as u64, false);
    out.push_str(",\"sites\":");
    list(out, &b.sites, budget_site);
    out.push('}');
}

fn budget_site(out: &mut String, s: &BudgetSite) {
    out.push('{');
    field_str(out, "function", &s.func, true);
    position(out, s.position, &s.file);
    out.push('}');
}

/// A model's answer arriving somewhere that changes something.
///
/// The member a rule is most likely to be written against, and the reason
/// `approved` is a boolean rather than an absent member when nobody was asked:
/// a rule that has to notice something missing is a rule that will not.
fn flow(out: &mut String, f: &crate::Flow) {
    out.push('{');
    field_str(out, "capability", &f.cap, true);
    field_str(out, "effect", kind_name(f.kind), false);
    field_str(out, "function", &f.func, false);
    position(out, f.position, &f.file);
    field_bool(out, "approved", f.approved, false);
    out.push('}');
}

fn reaches(out: &mut String, r: &Reaches) {
    out.push('{');
    field_str(out, "from", &r.from, true);
    field_str(out, "to", &r.to, false);
    field_bool(out, "spawned", r.spawned, false);
    out.push('}');
}

/// Where something is written, when the file has a debug section to say.
fn position(out: &mut String, at: Option<(u32, u32)>, file: &Option<String>) {
    match at {
        Some((line, col)) => {
            field_u64(out, "line", line as u64, false);
            field_u64(out, "column", col as u64, false);
        }
        None => {
            out.push_str(",\"line\":null,\"column\":null");
        }
    }
    optional_str(out, "file", file.as_deref());
}

fn kind_name(kind: sic_core::CapKind) -> &'static str {
    use sic_core::CapKind;
    match kind {
        CapKind::Read => "read",
        CapKind::Write => "write",
        CapKind::Exec => "exec",
        CapKind::Invoke => "invoke",
    }
}

fn answers_name(answers: sic_core::Answers) -> &'static str {
    use sic_core::Answers;
    match answers {
        Answers::Unsaid => "unsaid",
        Answers::Json => "json",
        Answers::Jsonl => "jsonl",
    }
}

fn nonzero(value: u32) -> Option<u64> {
    (value > 0).then_some(value as u64)
}

fn list<T>(out: &mut String, items: &[T], each: impl Fn(&mut String, &T)) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        each(out, item);
    }
    out.push(']');
}

fn field_u64(out: &mut String, name: &str, value: u64, first: bool) {
    if !first {
        out.push(',');
    }
    out.push_str(&format!("\"{name}\":{value}"));
}

fn field_bool(out: &mut String, name: &str, value: bool, first: bool) {
    if !first {
        out.push(',');
    }
    out.push_str(&format!("\"{name}\":{value}"));
}

fn field_str(out: &mut String, name: &str, value: &str, first: bool) {
    if !first {
        out.push(',');
    }
    out.push_str(&format!("\"{name}\":"));
    write_quoted(out, value);
}

fn optional_u64(out: &mut String, name: &str, value: Option<u64>) {
    match value {
        Some(v) => field_u64(out, name, v, false),
        None => out.push_str(&format!(",\"{name}\":null")),
    }
}

fn optional_str(out: &mut String, name: &str, value: Option<&str>) {
    match value {
        Some(v) => field_str(out, name, v, false),
        None => out.push_str(&format!(",\"{name}\":null")),
    }
}
