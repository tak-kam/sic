//! What a program may do, worked out without running it.
//!
//! Everything this needs is already in the bytecode, because each phase put it
//! there for exactly this: the manifest says what may be reached and what bounds
//! it, the policy table says how often and for how long, the type section says
//! what a validation will insist on, and the debug section maps it all back to
//! source.
//!
//! So this is a reader. It opens no socket, starts no process, and builds no
//! VM - which is what makes it safe to run on a program nobody has decided to
//! trust yet, the only time a plan is worth anything.
//!
//! It says what a program **may** do, not what it will. Working out which
//! effects are unavoidable takes dominance analysis over the control flow graph,
//! and a plan that says "these definitely, those maybe" is a different and more
//! useful thing than this. Claiming a certainty this cannot establish would be
//! worse than claiming none.

use sic_bytecode::inst::Op;
use std::collections::HashMap;

use sic_bytecode::program::Program;
use sic_core::{CapKind, Digest};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The digest of the bytecode this plan is of, so that approving a plan can
    /// be tied to running that exact program.
    pub digest: Digest,
    pub functions: Vec<FunctionPlan>,
    /// Every capability the module declares, whether or not it is called.
    pub capabilities: Vec<Grant>,
    /// The most calls the sites with a budget can make, summed. This is a real
    /// bound: a budget caps a site over the whole run.
    pub bounded_calls: u64,
    /// Call sites with no budget. How often one runs depends on the path taken
    /// and on recursion, so there is no number to give - saying so is better
    /// than inventing one.
    pub unbounded_sites: usize,
    /// Capabilities that are granted but never called.
    pub unused: Vec<String>,
    /// Whether the program was built from more than one file. A position is
    /// only worth a file name when there is a choice of file.
    pub multi_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPlan {
    pub name: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub pc: u32,
    /// The line and column this came from, when the file has a debug section.
    pub position: Option<(u32, u32)>,
    /// The file it came from, which is not always the one named on the command
    /// line.
    pub file: Option<String>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A capability call, with the manifest entry that allows it.
    Capability {
        name: String,
        kind: CapKind,
        constraint: String,
        /// The most times this site can run in a whole run, from its budget.
        /// `None` means it has no budget, and how often it runs depends on the
        /// path taken.
        budget: Option<u32>,
        /// How many times one visit to this site may call out, from `retry`.
        attempts: u32,
        timeout_ms: u32,
    },
    /// A document checked against a type.
    Verify {
        type_name: String,
    },
    Spawn {
        func: String,
    },
    Await,
}

impl Action {
    /// The word a plan leads a line with.
    pub fn verb(&self) -> &'static str {
        match self {
            // `process.capture` runs something, so its kind is `Exec`, but a
            // plan is read by a person deciding whether to run this - and
            // "reads what it says" is the part they need to see.
            Action::Capability { name, .. } if name == "process.capture" => "CAPTURE",
            Action::Capability { kind, .. } => match kind {
                CapKind::Read => "READ",
                CapKind::Write => "WRITE",
                CapKind::Exec => "EXEC",
                CapKind::Invoke => "INVOKE",
            },
            Action::Verify { .. } => "VERIFY",
            Action::Spawn { .. } => "SPAWN",
            Action::Await => "AWAIT",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub name: String,
    pub kind: CapKind,
    pub constraint: String,
    /// The digest the file has to have, if the grant pins what runs.
    pub pin: String,
    /// What a call's arguments have to start with. The arguments themselves
    /// are runtime values a plan cannot know, so this prefix is the only part
    /// of them it can report.
    pub args: Vec<String>,
    /// The files whose code calls it. Derived from the call sites rather than
    /// from a declaration, so it says where a grant is really used.
    pub called_from: Vec<String>,
}

/// Reads a program and works out what it may do.
pub fn plan(program: &Program, digest: Digest) -> Plan {
    let mut functions = Vec::new();
    let mut bounded_calls: u64 = 0;
    let mut unbounded_sites = 0usize;
    let mut called = vec![false; program.caps.len()];
    let mut call_sites: HashMap<String, Vec<String>> = HashMap::new();

    for func in &program.funcs {
        let mut steps = Vec::new();
        for offset in 0..func.code_len {
            let pc = func.code_off + offset;
            let Some(inst) = program.code.get(pc as usize) else {
                break;
            };
            let Some(op) = inst.op() else {
                continue;
            };
            let action = match op {
                Op::CallCap => {
                    let Some(cap) = program.caps.get(inst.b() as usize) else {
                        continue;
                    };
                    if let Some(slot) = called.get_mut(inst.b() as usize) {
                        *slot = true;
                    }
                    // Which file asks for a grant is the thing a reader most
                    // needs when a program is built from more than one: a
                    // manifest that does not say where its entries are used is
                    // approving something you cannot see.
                    if let Some(file) = program.debug.file(pc) {
                        let sites = call_sites.entry(cap.name.clone()).or_default();
                        if !sites.iter().any(|f| f == file) {
                            sites.push(file.to_string());
                        }
                    }
                    let policy = program.policy_at(pc);
                    let attempts = policy.map(|p| p.attempts.max(1)).unwrap_or(1);
                    // A budget is the only real bound: `retry` says how many
                    // times one visit may call out, and how many visits there
                    // are depends on the path taken and on recursion.
                    let budget = policy.map(|p| p.budget).filter(|b| *b > 0);
                    match budget {
                        Some(budget) => bounded_calls += budget as u64,
                        None => unbounded_sites += 1,
                    }
                    Action::Capability {
                        name: cap.name.clone(),
                        kind: cap.kind,
                        constraint: cap.constraints.clone(),
                        budget,
                        attempts,
                        timeout_ms: policy.map(|p| p.timeout_ms).unwrap_or(0),
                    }
                }
                Op::FromJson => Action::Verify {
                    type_name: program.type_name(inst.b() as u32),
                },
                Op::Spawn => Action::Spawn {
                    func: program
                        .funcs
                        .get(inst.b() as usize)
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| format!("f{}", inst.b())),
                },
                Op::Await => Action::Await,
                _ => continue,
            };
            steps.push(Step {
                pc,
                position: program.debug.position(pc),
                file: program.debug.file(pc).map(str::to_string),
                action,
            });
        }
        if !steps.is_empty() {
            functions.push(FunctionPlan {
                name: func.name.clone(),
                steps,
            });
        }
    }

    let capabilities: Vec<Grant> = program
        .caps
        .iter()
        .map(|c| Grant {
            name: c.name.clone(),
            kind: c.kind,
            constraint: c.constraints.clone(),
            pin: c.pin.clone(),
            args: c.args.clone(),
            called_from: call_sites.remove(&c.name).unwrap_or_default(),
        })
        .collect();
    let unused = program
        .caps
        .iter()
        .zip(&called)
        .filter(|(_, called)| !**called)
        .map(|(c, _)| c.name.clone())
        .collect();

    Plan {
        digest,
        functions,
        capabilities,
        bounded_calls,
        unbounded_sites,
        unused,
        multi_file: program.debug.sources.len() > 1,
    }
}

/// Renders a plan for a person to read.
pub fn render(plan: &Plan, source: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("Execution plan for {source}\n"));
    out.push_str(&format!("bytecode {}\n", plan.digest));

    if plan.functions.is_empty() {
        out.push_str("\n  (no external effects)\n");
    }
    for function in &plan.functions {
        out.push_str(&format!("\n  {}\n", function.name));
        for (i, step) in function.steps.iter().enumerate() {
            out.push_str(&format!("    {}. {:<9}", i + 1, step.action.verb()));
            match &step.action {
                Action::Capability {
                    name,
                    constraint,
                    budget,
                    attempts,
                    timeout_ms,
                    ..
                } => {
                    out.push_str(&format!("{name:<16}{constraint:?}"));
                    if let Some(budget) = budget {
                        out.push_str(&format!("  at most {budget} in a run"));
                    }
                    if *attempts > 1 {
                        out.push_str(&format!("  {attempts} attempts each"));
                    }
                    if *timeout_ms > 0 {
                        out.push_str(&format!("  within {timeout_ms}ms"));
                    }
                }
                Action::Verify { type_name } => out.push_str(type_name),
                Action::Spawn { func } => out.push_str(func),
                Action::Await => {}
            }
            if let Some((line, col)) = step.position {
                match (plan.multi_file, &step.file) {
                    (true, Some(file)) => out.push_str(&format!("   ; {file}:{line}:{col}")),
                    _ => out.push_str(&format!("   ; {line}:{col}")),
                }
            }
            out.push('\n');
        }
    }

    out.push_str("\nCapabilities:\n");
    if plan.capabilities.is_empty() {
        out.push_str("  (none)\n");
    }
    for grant in &plan.capabilities {
        out.push_str(&format!(
            "  {:<16}[{}]  {:?}",
            grant.name,
            grant.kind.name(),
            grant.constraint
        ));
        if !grant.args.is_empty() {
            let quoted: Vec<String> = grant.args.iter().map(|a| format!("{a:?}")).collect();
            out.push_str(&format!("  args [{}]", quoted.join(", ")));
        }
        // Whether a grant pins what runs is what a reader most wants to know
        // about it, so it is on the same line.
        if grant.pin.is_empty() {
            out.push_str("  (not pinned)");
        } else {
            out.push_str(&format!("  sha256:{}", grant.pin));
        }
        out.push('\n');
        if plan.multi_file && !grant.called_from.is_empty() {
            out.push_str(&format!(
                "    called from {}\n",
                grant.called_from.join(", ")
            ));
        }
    }
    for name in &plan.unused {
        out.push_str(&format!(
            "  warning: `{name}` is granted but never called\n"
        ));
    }

    // Only a budget bounds a site over a whole run, so only those are summed.
    // A site with no budget gets no number, because a number that ignores
    // recursion would be a guess dressed as a fact.
    out.push('\n');
    match (plan.bounded_calls, plan.unbounded_sites) {
        (0, 0) => out.push_str("No capability calls.\n"),
        (0, sites) => out.push_str(&format!(
            "{sites} capability call site(s), none with a budget, so how often \
             they run depends on the path taken.\n"
        )),
        (calls, 0) => out.push_str(&format!("At most {calls} capability call(s).\n")),
        (calls, sites) => out.push_str(&format!(
            "At most {calls} call(s) from budgeted sites, plus {sites} site(s) \
             with no budget.\n"
        )),
    }
    out
}

#[cfg(test)]
mod tests;
