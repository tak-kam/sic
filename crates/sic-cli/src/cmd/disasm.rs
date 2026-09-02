//! `sic disasm <FILE.sicb>`: print bytecode as instructions.

use crate::out::say;

use std::process::ExitCode;

use super::load_bytecode;

pub fn run(path: &str) -> ExitCode {
    match load_bytecode(path) {
        Ok(program) => {
            say!("{}", sic_bytecode::disassemble(&program));
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
