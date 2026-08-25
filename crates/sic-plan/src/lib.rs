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
use sic_core::{CapGrant, CapKind, Digest};

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
        /// How many alternatives a decision offers, for `human.choose`. Read
        /// from the `MAKE_LIST` that built the argument, because how many
        /// choices somebody will be asked to make between is the thing a plan
        /// is being read for.
        alternatives: Option<u32>,
        /// Whether the call continues a conversation rather than starting one.
        /// What an agent was told earlier shapes what it answers now, and a
        /// plan that did not say so would describe calls that look independent
        /// and are not.
        remembers: bool,
        /// How many of the agent's own tools this site allows in a whole run,
        /// and how long one answer may take. A reader deciding whether to run
        /// this needs both: the first is what stops a loop, the second is what
        /// stops a wait.
        tools: Option<u32>,
        deadline_ms: Option<u32>,
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
            // Its own verb, because it is its own authority: this one reads
            // what a program said whether or not the program worked.
            Action::Capability { name, .. } if name == "process.run" => "RUN",
            // Asking a person is not the same act as asking a model, and a
            // plan is read by the person who will be asked.
            Action::Capability { name, .. } if name == "human.choose" => "CHOOSE",
            Action::Capability { name, .. } if name == "human.approve" => "APPROVE",
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
    /// Whether the grant says performing this twice is the same as performing
    /// it once. Whoever reads a plan is the person who should be deciding that,
    /// which is why it is printed rather than only checked.
    pub repeatable: bool,
    /// Whether the grant says the agent answering this program's model calls
    /// may use it too. Printed for the same reason `repeatable` is: it is a
    /// claim the manifest makes that the language cannot check, so the person
    /// reading the plan is the one who has to decide it.
    pub delegable: bool,
    /// The directory a call runs in, or empty for the one `sic` was started
    /// in. Printed either way: a reader deciding whether to run this needs to
    /// know when the answer depends on their shell.
    pub dir: String,
    /// The environment a call is given. Empty means none.
    pub env: Vec<(String, String)>,
    /// The files whose code calls it. Derived from the call sites rather than
    /// from a declaration, so it says where a grant is really used.
    pub called_from: Vec<String>,
}

/// How many elements the list in `reg` was built from, if a `MAKE_LIST` in this
/// function built it.
///
/// A plan reads bytecode, so this is the only way to know how many alternatives
/// a decision offers: the options are a list the program builds just before it
/// asks. Anything else - a list passed in, or built in another function -
/// answers `None`, because a plan does not guess.
fn options_at(
    program: &Program,
    func: &sic_bytecode::FuncDef,
    call_pc: u32,
    reg: u8,
) -> Option<u32> {
    // Arguments are moved into a contiguous window before a call, so the list
    // was built somewhere else and copied here. Following the moves back to
    // whatever wrote it is the whole of the search.
    let mut reg = reg;
    let mut pc = call_pc;
    while pc > func.code_off {
        pc -= 1;
        let inst = program.code.get(pc as usize)?;
        if inst.a() != reg {
            continue;
        }
        match inst.op() {
            Some(Op::MakeList) => return Some(u32::from(inst.c())),
            Some(Op::Move) => reg = inst.b(),
            // Anything else built it, and a plan does not guess.
            _ => return None,
        }
    }
    None
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
                    let alternatives = if cap.name == "human.choose" {
                        options_at(program, func, pc, inst.c().saturating_add(1))
                    } else {
                        None
                    };
                    Action::Capability {
                        name: cap.name.clone(),
                        kind: cap.kind,
                        constraint: cap.constraints.clone(),
                        alternatives,
                        budget,
                        attempts,
                        timeout_ms: policy.map(|p| p.timeout_ms).unwrap_or(0),
                        remembers: policy.map(|p| p.conversation != 0).unwrap_or(false),
                        tools: policy.map(|p| p.tools).filter(|t| *t > 0),
                        deadline_ms: policy.map(|p| p.deadline_ms).filter(|d| *d > 0),
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
            repeatable: c.repeatable,
            delegable: c.delegable,
            dir: c.dir.clone(),
            env: c.env.clone(),
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
                    alternatives,
                    remembers,
                    tools,
                    deadline_ms,
                    ..
                } => {
                    out.push_str(&format!("{name:<16}{constraint:?}"));
                    if let Some(n) = alternatives {
                        out.push_str(&format!("  {n} options"));
                    }
                    if *remembers {
                        out.push_str("  in one conversation per task");
                    }
                    if let Some(budget) = budget {
                        out.push_str(&format!("  at most {budget} in a run"));
                    }
                    if *attempts > 1 {
                        out.push_str(&format!("  {attempts} attempts each"));
                    }
                    if *timeout_ms > 0 {
                        out.push_str(&format!("  within {timeout_ms}ms"));
                    }
                    // A model call is bounded by three numbers and gets all
                    // three whether or not the program set them: the driver
                    // has a fallback for the deadline and no limit at all for
                    // the tools. So the absences are printed rather than left
                    // out. A reader who is not told assumes what is on the
                    // line is the whole of it - which is the same reason a
                    // `process` grant prints its directory either way.
                    if name == "llm.invoke" {
                        match tools {
                            Some(tools) => out.push_str(&format!("  at most {tools} tool use(s)")),
                            None => out.push_str("  any number of tool uses"),
                        }
                        match deadline_ms {
                            Some(ms) => out.push_str(&format!("  {ms}ms per answer")),
                            None => out.push_str("  no deadline of its own"),
                        }
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
        // A child process depends on these whether or not the grant mentions
        // them, so the plan says which - a reader who is not told assumes the
        // grant is the whole of it.
        if grant.name.starts_with("process.") {
            match grant.dir.is_empty() {
                true => out.push_str("  in the directory `sic` is started in"),
                false => out.push_str(&format!("  in {:?}", grant.dir)),
            }
            match grant.env.is_empty() {
                true => out.push_str("  with no environment"),
                false => {
                    let names: Vec<&str> = grant.env.iter().map(|(n, _)| n.as_str()).collect();
                    out.push_str(&format!("  env {}", names.join(", ")));
                }
            }
        }
        if grant.delegable {
            out.push_str("  delegable");
        }
        if grant.repeatable {
            out.push_str("  repeatable");
        }
        out.push('\n');
        // The one grant whose answer comes from something that acts on its own.
        // It used to print a warning saying so, because there was nothing true
        // to print instead. There is now: the agent's authority is this same
        // manifest, so it is reported as a view of it rather than guessed at.
        // See `docs/design/authority.md` §10.
        if grant.name == "llm.invoke" {
            out.push_str(&agent_authority(&plan.capabilities));
        }
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

/// What the agent answering a model call may do, read from the same manifest.
///
/// Every line names **where** it is enforced, in parentheses, because a gate
/// and a boundary are different things and a reader deciding whether to run
/// this has to be able to tell them apart. A line with nothing in parentheses
/// would be a claim with no mechanism behind it.
fn agent_authority(manifest: &[Grant]) -> String {
    let grants: Vec<CapGrant> = manifest
        .iter()
        .map(|g| CapGrant {
            name: g.name.clone(),
            kind: g.kind,
            constraint: g.constraint.clone(),
            pin: g.pin.clone(),
            args: g.args.clone(),
            delegable: g.delegable,
            dir: g.dir.clone(),
            env: g.env.clone(),
        })
        .collect();

    // A manifest nothing can enforce stops the run before it starts, so a plan
    // of one says that rather than describing an agent that will never exist.
    if sic_core::authority_of(&grants).is_err() {
        return "    warning: this manifest cannot be enforced against an agent,\n\
                \x20            so a run that names a driver refuses to start\n"
            .to_string();
    }

    let mut out = String::new();

    for grant in &grants {
        match sic_core::reach_of(grant) {
            sic_core::Reach::Translated(rules) => {
                for rule in rules {
                    out.push_str(&line(
                        &format!("the agent's {}", rule.tool),
                        &format!("{:?}", grant.constraint),
                        "its own permissions",
                    ));
                }
            }
            sic_core::Reach::Routed(_) => {
                let how = match grant.pin.is_empty() {
                    true => "through the broker".to_string(),
                    false => format!(
                        "through the broker, sha256:{}",
                        &grant.pin[..8.min(grant.pin.len())]
                    ),
                };
                out.push_str(&line(
                    "the agent may use",
                    &format!("{:?}", grant.constraint),
                    &how,
                ));
            }
            // Said rather than left out. A reader deciding whether to run this
            // needs to know that the program may run something the agent may
            // not, which is the whole of what `delegable` is for.
            sic_core::Reach::Withheld(why) => {
                out.push_str(&line(
                    "the agent may not",
                    &format!("use {:?}", grant.constraint),
                    why,
                ));
            }
            sic_core::Reach::Summons | sic_core::Reach::Unenforceable(_) => {}
        }
    }

    // The three that are true of every agent, and are the reason the tool
    // surface is decided by the hook rather than by the rules.
    out.push_str(&line(
        "the agent may not",
        "reach the network",
        "no tool it has can",
    ));
    // "a shell of its own", because a `delegable` grant of one is a shell the
    // agent may use - through the broker, against this manifest, into this
    // journal. Both lines are about the hook's boundary, and saying only "run
    // a shell" made them read as a contradiction when a manifest had granted
    // one on purpose.
    out.push_str(&line(
        "the agent may not",
        "run a shell of its own",
        "refused by the hook",
    ));
    out.push_str(&line(
        "the agent may not",
        "use any other tool",
        "refused by the hook",
    ));
    out
}

fn line(what: &str, subject: &str, how: &str) -> String {
    format!(
        "    {what:<18} {subject:<24} ({how})
"
    )
}

#[cfg(test)]
mod tests;
