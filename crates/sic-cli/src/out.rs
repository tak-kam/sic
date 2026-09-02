//! Writing to standard output when the reader may have stopped reading.
//!
//! `println!` panics if the write fails, and the write fails whenever whatever
//! was reading closes the pipe first - `| head`, `| grep -q`, a pager somebody
//! quits out of. Rust's runtime ignores `SIGPIPE` at start-up so that no output
//! is lost silently, which is the right default for a program that must not
//! lose output and the wrong one for a program whose job is to print things a
//! person filters. The result was a panic message naming a file in the standard
//! library, a backtrace hint, and exit code 101 from a command that succeeded -
//! next to three exit codes this project gives documented meanings to.
//!
//! So a closed pipe is not an error here. It is the reader saying it has
//! enough, and the answer is to stop delivering and let the command finish with
//! the status its work earned. The alternative - ending the process from
//! wherever the write happened - would decide the exit code from inside a
//! printer, and a `sic run` that failed would report success because nobody was
//! reading.
//!
//! **Every other write error still panics.** A full disk while stdout is
//! redirected to a file is a lost result, not a reader who left, and silence
//! there would be the failure mode `println!` was cautious about in the first
//! place.
//!
//! `sayln!` and `say!` are the shapes `println!` and `print!` have, so a caller
//! writes what it would have written. `crates/sic-cli/tests/output.rs` refuses
//! the originals in this crate, because one that got missed is the panic back
//! in one place.

use std::io::Write;

/// Writes to standard output, treating a closed pipe as nothing to do.
pub fn write(args: std::fmt::Arguments<'_>) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    match handle.write_fmt(args) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("failed printing to stdout: {e}"),
    }
}

/// `println!`, for a reader that is allowed to leave.
macro_rules! sayln {
    () => { $crate::out::write(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::write(format_args!("{}\n", format_args!($($arg)*))) };
}

/// `print!`, likewise.
macro_rules! say {
    ($($arg:tt)*) => { $crate::out::write(format_args!($($arg)*)) };
}

pub(crate) use say;
pub(crate) use sayln;
