//! `sic plan <FILE>`: what a program may do, before it does any of it.
//!
//! Takes a `.sic` file, which it compiles first, or a `.sicb` directly. The
//! second form matters: the thing you plan should be the thing you run, and
//! bytecode that arrived from somewhere else has no source to consult.
//!
//! Nothing here runs the program. That is what makes a plan worth having on a
//! program nobody has decided to trust yet.
//!
//! `--graph` writes the same plan as Mermaid, which says the one thing a list
//! of functions cannot: which of them reach which. `--json` writes it as data,
//! for the readers that are not people - a rule about a repository, a diff of
//! what a branch may now do, anything that wants to sort or filter. All three
//! render one `Plan`, so none of them can say a program may do something
//! another of them does not. See `docs/design/plan.md`.

use crate::out::say;

use std::process::ExitCode;

use super::{EXIT_FAILURE, compile_source, load_bytecode};

/// Which of the three renderings was asked for. A plan is one thing; these are
/// ways of reading it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum As {
    Prose,
    Graph,
    Json,
}

pub fn run(path: &str, shape: As) -> ExitCode {
    let program = if path.ends_with(".sicb") {
        match load_bytecode(path) {
            Ok(program) => program,
            Err(code) => return code,
        }
    } else {
        match compile_source(path) {
            Ok(program) => program,
            Err(code) => return code,
        }
    };

    // A plan describes bytecode that has been checked. Planning something the
    // verifier would refuse would be describing a program that cannot run.
    let report = sic_verify::verify(&program);
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if !report.ok() {
        eprintln!("error: this bytecode does not verify, so there is nothing to plan");
        for e in &report.errors {
            eprintln!("  {e}");
        }
        return ExitCode::from(EXIT_FAILURE);
    }

    let digest = sic_bytecode::digest(&program);
    let plan = sic_plan::plan(&program, digest);
    match shape {
        As::Graph => say!("{}", sic_plan::graph(&plan, path)),
        As::Json => say!("{}", sic_plan::to_json(&plan)),
        As::Prose => say!("{}", sic_plan::render(&plan, path)),
    }
    ExitCode::SUCCESS
}
