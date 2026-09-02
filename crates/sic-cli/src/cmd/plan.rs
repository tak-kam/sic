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
//! of functions cannot: which of them reach which. See
//! `docs/design/plan.md`.

use crate::out::say;

use std::process::ExitCode;

use sic_core::Digest;

use super::{EXIT_FAILURE, compile_source, load_bytecode};

pub fn run(path: &str, graph: bool) -> ExitCode {
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

    let digest = Digest::of(&sic_bytecode::encode(&program));
    let plan = sic_plan::plan(&program, digest);
    match graph {
        true => say!("{}", sic_plan::graph(&plan, path)),
        false => say!("{}", sic_plan::render(&plan, path)),
    }
    ExitCode::SUCCESS
}
