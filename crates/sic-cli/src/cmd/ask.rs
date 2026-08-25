//! Reading an answer to a waiting run from the terminal.
//!
//! `docs/design/interactive.md`. The question is the text the broker already
//! produced - the same text `sic attach` prints, with `human.choose`'s
//! alternatives already numbered - so nothing here decides what to ask. What
//! it decides is what counts as an answer, and that is `parse_answer`'s
//! decision too: this reads a line and hands it over.

use std::io::{BufRead, Write};

use sic_bytecode::TypeDesc;
use sic_core::CapValue;

/// What came back from the terminal.
pub enum Answered {
    /// The value, and why - which is what `--because` would have said.
    With {
        value: CapValue,
        because: Option<String>,
    },
    /// End of input. Not an answer, and not an error either: the run is
    /// already saved, so this leaves it exactly where a non-interactive run
    /// would have.
    Nothing,
}

/// Asks the question, and keeps asking until there is an answer or the input
/// ends.
///
/// Re-asking rather than failing is the point of having a person there. A run
/// that ended because somebody typed `yes` where a `Bool` was wanted would
/// have to be picked up by hand, which is the situation the flag exists to
/// avoid.
pub fn ask(question: &str, tag: &TypeDesc) -> std::io::Result<Answered> {
    let stdin = std::io::stdin();
    read_answer(&mut stdin.lock(), &mut std::io::stderr(), question, tag)
}

/// The same, with the terminal handed in.
///
/// Two arguments rather than two globals so that what counts as an answer can
/// be tested without a tty, which is the part with decisions in it. The prompt
/// goes to stderr because stdout is what a run returns.
fn read_answer(
    input: &mut impl BufRead,
    out: &mut impl Write,
    question: &str,
    tag: &TypeDesc,
) -> std::io::Result<Answered> {
    // Once, however many attempts it takes: a `human.choose` question carries
    // its alternatives, and reprinting ten of them to say `yes` is not a Bool
    // buries the thing that went wrong.
    writeln!(out, "waiting: {question}")?;
    loop {
        write!(out, "answer ({}): ", tag.short_name())?;
        out.flush()?;
        let Some(line) = read_line(input)? else {
            writeln!(out)?;
            return Ok(Answered::Nothing);
        };
        let value = match super::drive::parse_answer(&line, tag) {
            Ok(value) => value,
            Err(message) => {
                writeln!(out, "  {message}")?;
                continue;
            }
        };
        // Asked for every capability rather than only the two where it reads
        // as a decision: one rule is easier to hold than a table of
        // exceptions, and `sic attach --because` is accepted for all of them.
        write!(out, "why (optional): ")?;
        out.flush()?;
        let because = read_line(input)?.filter(|why| !why.is_empty());
        return Ok(Answered::With { value, because });
    }
}

/// One line, with the newline taken off, or `None` at end of input.
///
/// The trailing `\r` goes too: a terminal on Windows sends one, and a `String`
/// that ends in it is not the string the person typed.
fn read_line(input: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let end = line.trim_end_matches(['\n', '\r']).len();
    line.truncate(end);
    Ok(Some(line))
}

/// Whether there is a terminal to ask.
///
/// Checked before the program runs rather than at the first question: a run
/// that performed three effects and then found nobody to ask is a run that has
/// to be picked up by hand, which is what the flag was for.
pub fn a_terminal_is_there() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(typed: &str, tag: &TypeDesc) -> Answered {
        answer_saying(typed, tag).0
    }

    /// The same code path, with the terminal replaced by a slice: what is
    /// under test is what counts as an answer, not what a tty is.
    fn answer_saying(typed: &str, tag: &TypeDesc) -> (Answered, String) {
        let mut input = typed.as_bytes();
        let mut out = Vec::new();
        let answered = read_answer(&mut input, &mut out, "[why] go on?", tag).unwrap();
        (answered, String::from_utf8(out).unwrap())
    }

    /// A value and a reason, which is what `sic attach --value V --because W`
    /// would have recorded.
    #[test]
    fn a_line_and_then_a_reason() {
        let Answered::With { value, because } = answer("true\ntests pass\n", &TypeDesc::Bool)
        else {
            panic!("an answer");
        };
        assert_eq!(value, CapValue::Bool(true));
        assert_eq!(because.as_deref(), Some("tests pass"));
    }

    /// An empty reason is no reason, not an empty one: `--because ""` is not
    /// what somebody pressing Enter meant.
    #[test]
    fn an_empty_reason_is_no_reason() {
        let Answered::With { because, .. } = answer("false\n\n", &TypeDesc::Bool) else {
            panic!("an answer");
        };
        assert_eq!(because, None);
    }

    /// Wrong once is not fatal. The run is saved and the person is present, so
    /// there is nothing to gain by making them start the command again.
    #[test]
    fn an_answer_that_does_not_parse_is_asked_again() {
        let Answered::With { value, .. } = answer("yes\ntrue\n\n", &TypeDesc::Bool) else {
            panic!("an answer");
        };
        assert_eq!(value, CapValue::Bool(true));
    }

    /// And the person is told why it was not an answer.
    #[test]
    fn a_bad_answer_says_what_was_wanted() {
        let (_, said) = answer_saying("yes\ntrue\n\n", &TypeDesc::Bool);
        assert!(said.contains("`yes` is not `true` or `false`"), "{said}");
    }

    /// `human.choose` answers with an index, and the question already carries
    /// the numbered alternatives.
    #[test]
    fn a_decision_is_answered_by_its_number() {
        let Answered::With { value, .. } = answer("2\n\n", &TypeDesc::Int) else {
            panic!("an answer");
        };
        assert_eq!(value, CapValue::I64(2));
    }

    /// End of input is not an answer. The caller leaves the run waiting, which
    /// is where it already is.
    #[test]
    fn the_input_ending_is_not_an_answer() {
        assert!(matches!(answer("", &TypeDesc::Bool), Answered::Nothing));
    }

    /// And ending after the value, before the reason, is not half an answer.
    #[test]
    fn the_input_ending_before_the_reason_leaves_no_reason() {
        let Answered::With { value, because } = answer("true", &TypeDesc::Bool) else {
            panic!("an answer");
        };
        assert_eq!(value, CapValue::Bool(true));
        assert_eq!(because, None);
    }

    /// The question is printed once, not once per attempt: it can be a
    /// `human.choose` with ten alternatives under it.
    #[test]
    fn the_question_is_asked_once_however_many_answers_it_takes() {
        let (_, said) = answer_saying("no\nnope\ntrue\n\n", &TypeDesc::Bool);
        assert_eq!(said.matches("go on?").count(), 1, "{said}");
    }

    /// A terminal on Windows ends a line with two characters, and neither of
    /// them is part of what was typed.
    #[test]
    fn a_carriage_return_is_not_part_of_the_answer() {
        let Answered::With { value, because } = answer("hello\r\nbecause\r\n", &TypeDesc::Str)
        else {
            panic!("an answer");
        };
        assert_eq!(value, CapValue::Str("hello".to_string()));
        assert_eq!(because.as_deref(), Some("because"));
    }
}
