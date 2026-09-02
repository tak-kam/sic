//! What the two processes say to each other.
//!
//! `docs/design/processes.md` §5. The capability types cross a boundary
//! between two *crates* and live in `sic-core`, which is what makes that
//! boundary checkable; this is the transport between two `sic` processes, and
//! both ends of it are this binary.
//!
//! Six message kinds, four of them one way. Nothing here is stored, so there
//! is no file format to keep: a `sic` talking to a `sic` that is not this one
//! meets a tag it does not know and says so.

use sic_core::{BinError, CapError, CapOutcome, CapRequest, CapValue, Reader, Writer};
use sic_journal::Event;

type Result<T> = std::result::Result<T, BinError>;

/// The largest message either side will read.
///
/// A length prefix is a promise, and this is what stops one from being
/// believed. Larger than `route`'s because a program crosses this one, and a
/// program is the biggest thing that does.
pub const MAX_FRAME: u32 = 64 << 20;

/// What the parent tells the child.
#[derive(Debug, Clone, PartialEq)]
pub enum ToVm {
    /// The bytecode, and where to start in it.
    ///
    /// The program crosses rather than a path: the child is the side that is
    /// meant to be able to reach less, and handing it a filename would be
    /// handing it a reason to open one.
    Start {
        program: Vec<u8>,
        entry: u32,
        fuel: u64,
        run: u128,
    },
    /// What a capability call answered, or why it did not.
    Answer(std::result::Result<CapOutcome, CapError>),
    /// A run that already exists, picked up where it stopped.
    ///
    /// The checkpoint carries its own fuel, run id and journal position, so
    /// none of `Start`'s fields is repeated here. What the parent has to send
    /// after it is the answer: `Resume`.
    Restore {
        program: Vec<u8>,
        checkpoint: Vec<u8>,
    },
    /// The value a person or an agent supplied for the call the run stopped
    /// at. Used by `resume` and `attach`; `run` never sends one.
    Resume(CapValue),
}

/// What the child tells the parent.
#[derive(Debug, Clone, PartialEq)]
pub enum FromVm {
    /// One journal event, whole - see `sic_journal::wire` for why not JSON.
    Event(Event),
    /// A capability call for the broker.
    Request(CapRequest),
    /// A checkpoint, as bytes. The child holds the state and the parent holds
    /// the filesystem, so the child produces these and never writes one.
    Checkpoint(Vec<u8>),
    /// How the run ended. The last thing the child says.
    Ended(Ended),
}

/// How a run ended, in the words the parent can use.
///
/// Rendered rather than structured, and that is the one place this protocol
/// gives something up: the value and the failure live in the child's arena,
/// which means nothing on the other side of a wire - `CapValue` exists exactly
/// because a `Value` does not survive leaving. The child renders them, the
/// parent prints them.
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    /// What the run returned, as `Vm::display` writes it. Empty for unit,
    /// which is the case `finish` prints nothing for.
    Finished(String),
    /// What went wrong, as `report_failure` would have written it.
    Failed(String),
    /// What the run is waiting for. A checkpoint has already been sent.
    Suspended(String),
}

impl ToVm {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            ToVm::Start {
                program,
                entry,
                fuel,
                run,
            } => {
                w.u8(0);
                w.u32(program.len() as u32);
                w.bytes(program);
                w.u32(*entry);
                w.u64(*fuel);
                w.u128(*run);
            }
            ToVm::Answer(Ok(CapOutcome::Value(value))) => {
                w.u8(1);
                value.write(&mut w);
            }
            ToVm::Answer(Ok(CapOutcome::Deferred { question })) => {
                w.u8(2);
                w.str(question);
            }
            ToVm::Answer(Err(error)) => {
                w.u8(3);
                w.str(&error.message);
            }
            ToVm::Resume(value) => {
                w.u8(4);
                value.write(&mut w);
            }
            ToVm::Restore {
                program,
                checkpoint,
            } => {
                w.u8(5);
                w.u32(program.len() as u32);
                w.bytes(program);
                w.u32(checkpoint.len() as u32);
                w.bytes(checkpoint);
            }
        }
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<ToVm> {
        let mut r = Reader::new(bytes);
        Ok(match r.u8()? {
            0 => {
                // One byte is the smallest a program can be, which is what
                // stops a claimed length from allocating on a promise.
                let len = r.count(1)?;
                ToVm::Start {
                    program: r.take(len)?.to_vec(),
                    entry: r.u32()?,
                    fuel: r.u64()?,
                    run: r.u128()?,
                }
            }
            1 => ToVm::Answer(Ok(CapOutcome::Value(CapValue::read(&mut r)?))),
            2 => ToVm::Answer(Ok(CapOutcome::Deferred { question: r.str()? })),
            3 => ToVm::Answer(Err(CapError::new(r.str()?))),
            4 => ToVm::Resume(CapValue::read(&mut r)?),
            5 => {
                let len = r.count(1)?;
                let program = r.take(len)?.to_vec();
                let len = r.count(1)?;
                ToVm::Restore {
                    program,
                    checkpoint: r.take(len)?.to_vec(),
                }
            }
            other => return Err(BinError::new(format!("unknown message tag {other}"))),
        })
    }
}

impl FromVm {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match self {
            FromVm::Event(event) => {
                w.u8(0);
                event.write(&mut w);
            }
            FromVm::Request(request) => {
                w.u8(1);
                request.write(&mut w);
            }
            FromVm::Checkpoint(bytes) => {
                w.u8(2);
                w.u32(bytes.len() as u32);
                w.bytes(bytes);
            }
            FromVm::Ended(ended) => {
                w.u8(3);
                match ended {
                    Ended::Finished(text) => {
                        w.u8(0);
                        w.str(text);
                    }
                    Ended::Failed(text) => {
                        w.u8(1);
                        w.str(text);
                    }
                    Ended::Suspended(question) => {
                        w.u8(2);
                        w.str(question);
                    }
                }
            }
        }
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<FromVm> {
        let mut r = Reader::new(bytes);
        Ok(match r.u8()? {
            0 => FromVm::Event(Event::read(&mut r)?),
            1 => FromVm::Request(CapRequest::read(&mut r)?),
            2 => {
                let len = r.count(1)?;
                FromVm::Checkpoint(r.take(len)?.to_vec())
            }
            3 => FromVm::Ended(match r.u8()? {
                0 => Ended::Finished(r.str()?),
                1 => Ended::Failed(r.str()?),
                2 => Ended::Suspended(r.str()?),
                other => return Err(BinError::new(format!("unknown ending tag {other}"))),
            }),
            other => return Err(BinError::new(format!("unknown message tag {other}"))),
        })
    }
}

// ---- framing ----
//
// The same shape `sic-broker::route` uses, and for the same reason: a length
// prefix that is checked before it is believed. Written again rather than
// shared because `route` is the agent's socket and this is the VM's, and one
// helper serving two protocols is how the maximum frame of one ends up
// bounding the other.

use std::io::{Read, Write};

/// Writes one message.
pub fn send(out: &mut impl Write, body: &[u8]) -> std::io::Result<()> {
    out.write_all(&(body.len() as u32).to_le_bytes())?;
    out.write_all(body)?;
    out.flush()
}

/// Reads one message, or `None` when the other end has finished.
///
/// A frame larger than `MAX_FRAME` is an error rather than an allocation: the
/// length is what the other end claims, and nothing has been read to check it
/// against yet.
pub fn recv(input: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        // The far side closed cleanly between messages, which is how a child
        // that has said everything it has to say ends.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(BinError::new(format!("cannot read a message: {e}"))),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(BinError::new(format!(
            "a message claims {len} bytes, over the {MAX_FRAME} byte limit"
        )));
    }
    let mut body = vec![0u8; len as usize];
    input
        .read_exact(&mut body)
        .map_err(|e| BinError::new(format!("a message ended early: {e}")))?;
    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sic_core::Digest;
    use sic_journal::{EventKind, LogLevel, RunId, SpanId, TaskId};

    fn every_to_vm() -> Vec<ToVm> {
        vec![
            ToVm::Start {
                program: vec![0, 1, 2, 250],
                entry: 7,
                fuel: 10_000_000,
                run: 0x1234_5678_9abc_def0_1234_5678_9abc_def0,
            },
            ToVm::Answer(Ok(CapOutcome::Value(CapValue::Unit))),
            ToVm::Answer(Ok(CapOutcome::Value(CapValue::Exit {
                code: -1,
                output: "two findings\n".into(),
            }))),
            ToVm::Answer(Ok(CapOutcome::Deferred {
                question: "deploy build 42?".into(),
            })),
            ToVm::Answer(Err(CapError::new("`/bin/x` is not an absolute path"))),
            ToVm::Resume(CapValue::Bool(true)),
            ToVm::Restore {
                program: vec![7, 8, 9],
                checkpoint: vec![1u8; 300],
            },
        ]
    }

    fn event(kind: EventKind) -> Event {
        Event {
            seq: 9,
            run: RunId(0x1234),
            task: TaskId(2),
            span: SpanId(5),
            parent: Some(SpanId(1)),
            kind,
        }
    }

    fn every_from_vm() -> Vec<FromVm> {
        let d = Digest::of(b"x");
        vec![
            FromVm::Event(event(EventKind::TaskAbandoned)),
            FromVm::Event(event(EventKind::CapabilityRequested {
                cap: "fs.read".into(),
                args: d,
                attempt: 2,
            })),
            FromVm::Request(CapRequest {
                index: 0,
                name: "fs.read".into(),
                args: vec![CapValue::Str("./a.txt".into())],
                task: 1,
                attempt: 1,
                timeout_ms: 0,
                conversation: 3,
                tools_left: Some(20),
                answer_ms: 300_000,
                rejected: String::new(),
            }),
            FromVm::Checkpoint(vec![9u8; 300]),
            FromVm::Ended(Ended::Finished("42".into())),
            FromVm::Ended(Ended::Failed("division by zero".into())),
            FromVm::Ended(Ended::Suspended("deploy build 42?".into())),
        ]
    }

    #[test]
    fn every_message_survives_the_wire() {
        for message in every_to_vm() {
            let bytes = message.to_bytes();
            assert_eq!(ToVm::from_bytes(&bytes), Ok(message), "{bytes:?}");
        }
        for message in every_from_vm() {
            let bytes = message.to_bytes();
            assert_eq!(FromVm::from_bytes(&bytes), Ok(message), "{bytes:?}");
        }
    }

    /// The reason this protocol has its own event codec rather than the
    /// journal's JSON: a `Logged` message reaches a file as its digest,
    /// because a file is the run's account. A sink is what decides whether a
    /// person sees the text, and across a wire the sink is on the other side.
    #[test]
    fn a_logged_message_crosses_as_text_and_not_as_a_digest() {
        let message = FromVm::Event(event(EventKind::Logged {
            level: LogLevel::Warn,
            message: "they failed, asking".into(),
        }));
        let bytes = message.to_bytes();
        assert_eq!(FromVm::from_bytes(&bytes), Ok(message));
        // In the file it would be a digest, and this is what makes the two
        // codecs two rather than one.
        let json = sic_journal::json::event_to_json(&event(EventKind::Logged {
            level: LogLevel::Warn,
            message: "they failed, asking".into(),
        }));
        assert!(!json.contains("they failed"), "{json}");
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed() {
        assert!(ToVm::from_bytes(&[]).is_err());
        assert!(ToVm::from_bytes(&[200]).is_err());
        assert!(FromVm::from_bytes(&[200]).is_err());
        // A `Start` whose length is a promise nothing backs.
        let mut w = Writer::new();
        w.u8(0);
        w.u32(u32::MAX);
        assert!(ToVm::from_bytes(&w.finish()).is_err());
    }

    /// The round trip the two processes will actually do, over the socket they
    /// will actually use. No command yet: this is the protocol on its own.
    #[test]
    fn the_two_sides_talk_over_a_socket() {
        use std::os::unix::net::UnixStream;

        let (mut parent, mut child) = UnixStream::pair().expect("a socketpair");

        let asked = FromVm::Request(CapRequest {
            index: 0,
            name: "fs.read".into(),
            args: vec![CapValue::Str("./a.txt".into())],
            task: 0,
            attempt: 1,
            timeout_ms: 0,
            conversation: 0,
            tools_left: None,
            answer_ms: 0,
            rejected: String::new(),
        });
        send(&mut child, &asked.to_bytes()).expect("the child asks");
        let heard = recv(&mut parent).expect("readable").expect("a message");
        assert_eq!(FromVm::from_bytes(&heard), Ok(asked));

        let answer = ToVm::Answer(Ok(CapOutcome::Value(CapValue::Str("hello".into()))));
        send(&mut parent, &answer.to_bytes()).expect("the parent answers");
        let heard = recv(&mut child).expect("readable").expect("a message");
        assert_eq!(ToVm::from_bytes(&heard), Ok(answer));

        // A child that has said everything closes, and the parent reads the
        // end rather than an error.
        send(
            &mut child,
            &FromVm::Ended(Ended::Finished("42".into())).to_bytes(),
        )
        .unwrap();
        drop(child);
        assert!(recv(&mut parent).unwrap().is_some());
        assert_eq!(recv(&mut parent).unwrap(), None);
    }
}
