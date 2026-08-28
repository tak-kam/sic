use sic_core::{CapValue, Digest};

use super::*;

#[test]
fn sequence_numbers_are_monotonic_and_spans_are_unique() {
    let mut journal = Journal::new(RunId(7), Box::new(MemorySink::default()));
    let root = journal.new_span();
    let child = journal.new_span();
    assert_ne!(root, child);

    journal.emit(
        root,
        None,
        EventKind::RunStarted {
            workflow: "main".into(),
            args: Digest::of(b""),
        },
    );
    journal.emit(
        child,
        Some(root),
        EventKind::FunctionEntered {
            func: "main".into(),
        },
    );
    assert_eq!(journal.count(), 2);
}

#[test]
fn events_carry_the_run_and_the_parent_span() {
    let mut sink = MemorySink::default();
    // The sink is moved into the journal, so the events come back out through
    // a second one for inspection.
    sink.emit(&Event {
        seq: 0,
        run: RunId(1),
        task: TaskId(0),
        span: SpanId(0),
        parent: None,
        kind: EventKind::RunFailed {
            error: "boom".into(),
        },
    });
    assert_eq!(sink.events.len(), 1);
    assert_eq!(sink.events[0].run, RunId(1));
}

#[test]
fn a_discarding_journal_still_numbers_its_events() {
    // Recording nothing must not change what the numbers would have been, so
    // that turning recording on does not change the run.
    let mut journal = Journal::discard();
    let span = journal.new_span();
    journal.emit(
        span,
        None,
        EventKind::FunctionExited {
            func: "main".into(),
        },
    );
    assert_eq!(journal.count(), 1);
}

#[test]
fn digests_distinguish_different_argument_lists() {
    // Length prefixes are what stop ["ab"] and ["a","b"] from colliding.
    let joined = digest_values(&[CapValue::Str("ab".into())]);
    let split = digest_values(&[CapValue::Str("a".into()), CapValue::Str("b".into())]);
    assert_ne!(joined, split);

    // The same values give the same digest, which is what makes a journal
    // comparable between runs.
    assert_eq!(
        digest_values(&[CapValue::I64(1), CapValue::Bool(true)]),
        digest_values(&[CapValue::I64(1), CapValue::Bool(true)])
    );
    assert_ne!(
        digest_values(&[CapValue::I64(1)]),
        digest_values(&[CapValue::Bool(true)])
    );
}

#[test]
fn a_digest_does_not_reveal_the_value() {
    // The point of recording a digest is that the value cannot be read back
    // out of the journal.
    let secret = CapValue::Str("hunter2".into());
    let line = json::event_to_json(&Event {
        seq: 0,
        run: RunId(0),
        task: TaskId(0),
        span: SpanId(0),
        parent: None,
        kind: EventKind::CapabilityRequested {
            cap: "fs.write".into(),
            args: digest_values(std::slice::from_ref(&secret)),
            attempt: 1,
        },
    });
    assert!(!line.contains("hunter2"), "{line}");
}

/// An argument vector is one value, and the journal has to be able to tell one
/// from another: a run that ran `git commit` and one that ran `git push` are
/// not the same run.
#[test]
fn digests_distinguish_argument_vectors() {
    let commit = digest_values(&[
        CapValue::Str("/usr/bin/git".into()),
        CapValue::List(vec!["commit".into()]),
    ]);
    let push = digest_values(&[
        CapValue::Str("/usr/bin/git".into()),
        CapValue::List(vec!["push".into()]),
    ]);
    assert_ne!(commit, push);

    // And a vector is not the strings it holds, laid out flat.
    assert_ne!(
        digest_values(&[CapValue::List(vec!["a".into(), "b".into()])]),
        digest_values(&[CapValue::Str("a".into()), CapValue::Str("b".into())])
    );
    assert_ne!(
        digest_values(&[CapValue::List(vec!["ab".into()])]),
        digest_values(&[CapValue::List(vec!["a".into(), "b".into()])])
    );
}

/// The one event a journal file does not hold as the VM emitted it, and so
/// the one thing anything comparing the two forms has to know.
///
/// `sic replay` compares a journal read back off a disk with the events a
/// second run of the same bytecode produced. Every other event survives that
/// round trip, so this is where the two forms meet.
#[test]
fn a_logged_message_is_recorded_as_its_digest() {
    let emitted = EventKind::Logged {
        level: LogLevel::Warn,
        message: "they failed, asking".into(),
    };
    assert_eq!(
        *emitted.as_recorded(),
        EventKind::Logged {
            level: LogLevel::Warn,
            message: Digest::of(b"they failed, asking").to_string(),
        }
    );

    // And it is what the file says, because one function writes both. A
    // reader and a writer that disagreed here made every replay of every
    // program that logs report a difference - issue #82.
    let line = json::event_to_json(&Event {
        seq: 0,
        run: RunId(0),
        task: TaskId(0),
        span: SpanId(0),
        parent: None,
        kind: emitted,
    });
    assert!(
        line.contains(&Digest::of(b"they failed, asking").to_string()),
        "{line}"
    );
    assert!(!line.contains("they failed"), "{line}");
}

/// Every other event is already what the file holds, so putting one in the
/// file's form is the identity and borrows rather than copies.
#[test]
fn nothing_else_changes_on_the_way_into_a_journal() {
    let completed = EventKind::CapabilityCompleted {
        cap: "fs.read".into(),
        result: Digest::of(b"a"),
        attempt: 1,
    };
    assert!(matches!(
        completed.as_recorded(),
        std::borrow::Cow::Borrowed(_)
    ));
    assert_eq!(*completed.as_recorded(), completed);
}
