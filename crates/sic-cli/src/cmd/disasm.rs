//! `sic disasm <FILE.sicb>`: print bytecode as instructions.

use std::process::ExitCode;

use super::load_bytecode;

pub fn run(path: &str) -> ExitCode {
    match load_bytecode(path) {
        Ok(program) => {
            print!("{}", sic_bytecode::disassemble(&program));
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}
