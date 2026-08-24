use sic_core::{CapGrant, CapKind, CapOutcome, CapRequest, CapValue};

use super::*;

fn grant(name: &str, kind: CapKind, constraint: &str) -> CapGrant {
    CapGrant {
        name: name.into(),
        kind,
        constraint: constraint.into(),
        pin: String::new(),
        args: Vec::new(),
    }
}

fn pinned(name: &str, kind: CapKind, constraint: &str, pin: &str) -> CapGrant {
    CapGrant {
        pin: pin.into(),
        args: Vec::new(),
        ..grant(name, kind, constraint)
    }
}

/// The sha256 of a file, as the broker computes it.
fn digest_of(path: &str) -> String {
    let mut hash = sic_core::Sha256::new();
    hash.update(&std::fs::read(path).expect("the file should be readable"));
    hash.finish().hex()
}

fn request(index: u32, name: &str, args: &[&str]) -> CapRequest {
    CapRequest {
        index,
        name: name.into(),
        args: args.iter().map(|a| CapValue::Str((*a).into())).collect(),
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        tools_left: 0,
        answer_ms: 0,
    }
}

/// A path in the temporary directory, unique to this process.
fn temp_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("sic-broker-{}-{name}", std::process::id()));
    p.to_str().expect("a UTF-8 temporary path").to_string()
}

#[test]
fn reads_and_writes_the_granted_path() {
    let path = temp_path("io.txt");
    let mut broker = Broker::new(vec![
        grant("fs.write", CapKind::Write, &path),
        grant("fs.read", CapKind::Read, &path),
    ]);

    let written = broker.call(&request(0, "fs.write", &[&path, "hello"]));
    assert_eq!(written, Ok(CapOutcome::Value(CapValue::Unit)));

    let read = broker.call(&request(1, "fs.read", &[&path]));
    assert_eq!(read, Ok(CapOutcome::Value(CapValue::Str("hello".into()))));

    std::fs::remove_file(&path).ok();
}

#[test]
fn refuses_a_path_the_grant_does_not_cover() {
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, "./allowed.txt")]);
    let err = broker
        .call(&request(0, "fs.read", &["./other.txt"]))
        .unwrap_err();
    assert!(err.message.contains("may only be used with"), "{err}");
}

#[test]
fn refuses_a_path_containing_a_parent_component() {
    // Even when it is textually the granted path: `..` is refused first.
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, "./a/../b.txt")]);
    let err = broker
        .call(&request(0, "fs.read", &["./a/../b.txt"]))
        .unwrap_err();
    assert!(err.message.contains("contains `..`"), "{err}");
}

#[test]
fn refuses_a_request_whose_name_disagrees_with_its_index() {
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, "./a.txt")]);
    let err = broker
        .call(&request(0, "process.exec", &["./a.txt"]))
        .unwrap_err();
    assert!(err.message.contains("but the request says"), "{err}");
}

#[test]
fn refuses_an_index_that_is_not_in_the_manifest() {
    let mut broker = Broker::new(Vec::new());
    let err = broker
        .call(&request(0, "fs.read", &["./a.txt"]))
        .unwrap_err();
    assert!(err.message.contains("no capability 0"), "{err}");
}

#[test]
fn refuses_a_relative_executable() {
    // No PATH search: what runs is decided by the grant.
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, "true")]);
    let err = broker
        .call(&request(0, "process.exec", &["true"]))
        .unwrap_err();
    assert!(err.message.contains("does not search PATH"), "{err}");
}

#[test]
fn refuses_arguments_of_the_wrong_shape() {
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, "./a.txt")]);

    let too_many = CapRequest {
        index: 0,
        name: "fs.read".into(),
        args: vec![CapValue::Str("./a.txt".into()), CapValue::I64(1)],
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        tools_left: 0,
        answer_ms: 0,
    };
    assert!(
        broker
            .call(&too_many)
            .unwrap_err()
            .message
            .contains("takes 1")
    );

    let wrong_type = CapRequest {
        index: 0,
        name: "fs.read".into(),
        args: vec![CapValue::I64(1)],
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        tools_left: 0,
        answer_ms: 0,
    };
    assert!(
        broker
            .call(&wrong_type)
            .unwrap_err()
            .message
            .contains("must be a String")
    );
}

#[test]
fn a_large_file_is_refused_rather_than_read() {
    let path = temp_path("big.txt");
    std::fs::write(&path, vec![b'x'; MAX_READ_BYTES as usize + 1]).unwrap();
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, &path)]);
    let err = broker.call(&request(0, "fs.read", &[&path])).unwrap_err();
    assert!(err.message.contains("over the"), "{err}");
    std::fs::remove_file(&path).ok();
}

#[test]
fn runs_a_granted_executable() {
    // Skip where the usual no-op binary is absent rather than fail.
    let Some(path) = ["/bin/true", "/usr/bin/true"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        return;
    };
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, path)]);
    assert_eq!(
        broker.call(&request(0, "process.exec", &[path])),
        Ok(CapOutcome::Value(CapValue::I64(0)))
    );
}

#[test]
fn reports_a_nonzero_exit_code() {
    let Some(path) = ["/bin/false", "/usr/bin/false"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
    else {
        return;
    };
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, path)]);
    match broker.call(&request(0, "process.exec", &[path])) {
        Ok(CapOutcome::Value(CapValue::I64(code))) => assert_ne!(code, 0),
        other => panic!("expected an exit code, got {other:?}"),
    }
}

#[test]
fn an_approval_defers_rather_than_answering() {
    // A person is not in this process. The call cannot complete, so the run has
    // to be able to stop and come back.
    let mut broker = Broker::new(vec![grant(
        "human.approve",
        CapKind::Invoke,
        "deploy to production",
    )]);
    match broker.call(&request(0, "human.approve", &["proceed?"])) {
        Ok(CapOutcome::Deferred { question }) => {
            // The grant travels with the question, so whoever answers can see
            // which grant is being exercised.
            assert_eq!(question, "[deploy to production] proceed?");
        }
        other => panic!("expected a deferral, got {other:?}"),
    }
}

/// Serializes the tests that write and then execute a script.
///
/// Tests run in parallel, and `Command::spawn` forks: a child started by one
/// test inherits whatever file descriptors are open at that instant, including
/// the write handle another test just opened. The kernel then refuses to
/// execute that file with `ETXTBSY`. Naming each script differently is not
/// enough, because the race is about when the fork happens rather than about
/// which file it is.
#[cfg(unix)]
static SCRIPTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
fn scripts_lock() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding it poisoned nothing worth protecting.
    SCRIPTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Calls the broker, waiting out a file that is briefly busy.
///
/// This is the same race as above, from the other side: it can still be lost
/// against a test that does not touch scripts at all.
#[cfg(unix)]
fn call_settled(broker: &mut Broker, request: &CapRequest) -> Result<CapOutcome, CapError> {
    for _ in 0..50 {
        match broker.call(request) {
            Err(e) if e.message.contains("Text file busy") => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return other,
        }
    }
    broker.call(request)
}

/// Writes an executable script and returns its path.
///
/// Each test uses its own name: they run in parallel, and a shared file would
/// be busy or already deleted.
#[cfg(unix)]
fn script(name: &str, body: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;

    if !std::path::Path::new("/bin/sh").exists() {
        return None;
    }
    let path = temp_path(name);
    std::fs::write(&path, body).ok()?;
    let mut perms = std::fs::metadata(&path).ok()?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).ok()?;
    Some(path)
}

/// A script that runs longer than any deadline a test sets.
///
/// `process.exec` takes no arguments yet, so a slow child has to be a file
/// rather than `sleep 5`.
#[cfg(unix)]
fn slow_script(name: &str) -> Option<String> {
    // Long enough to outlast a 100ms deadline by a wide margin, short enough
    // that the test which waits for it does not cost five seconds every run.
    script(name, "#!/bin/sh\nsleep 1\n")
}

#[cfg(unix)]
#[test]
fn a_deadline_kills_a_slow_child() {
    let _serialized = scripts_lock();
    let Some(script) = slow_script("deadline.sh") else {
        return;
    };
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, &script)]);
    let mut req = request(0, "process.exec", &[&script]);
    req.timeout_ms = 100;

    let started = std::time::Instant::now();
    let err = call_settled(&mut broker, &req).unwrap_err();
    assert!(err.message.contains("did not finish within"), "{err}");
    // The child was killed rather than waited out.
    assert!(started.elapsed() < std::time::Duration::from_secs(4));

    std::fs::remove_file(&script).ok();
}

#[cfg(unix)]
#[test]
fn without_a_deadline_a_slow_child_is_waited_for() {
    // The same script, with no timeout, runs to completion.
    let _serialized = scripts_lock();
    let Some(script) = slow_script("waited.sh") else {
        return;
    };
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, &script)]);
    match call_settled(&mut broker, &request(0, "process.exec", &[&script])) {
        Ok(CapOutcome::Value(CapValue::I64(code))) => assert_eq!(code, 0),
        other => panic!("expected an exit code, got {other:?}"),
    }
    std::fs::remove_file(&script).ok();
}

#[test]
fn a_capability_that_cannot_honour_a_deadline_says_so() {
    // Ignoring a timeout would tell the program the call was bounded when it
    // was not.
    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, "./a.txt")]);
    let mut req = request(0, "fs.read", &["./a.txt"]);
    req.timeout_ms = 100;
    let err = broker.call(&req).unwrap_err();
    assert!(err.message.contains("cannot honour a timeout"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_pinned_executable_runs_only_if_it_is_the_pinned_one() {
    // A path says where to look, not what is there.
    let _serialized = scripts_lock();
    let Some(script) = script("pinned.sh", "#!/bin/sh\nexit 0\n") else {
        return;
    };
    let digest = digest_of(&script);

    let mut broker = Broker::new(vec![pinned(
        "process.exec",
        CapKind::Exec,
        &script,
        &digest,
    )]);
    assert_eq!(
        call_settled(&mut broker, &request(0, "process.exec", &[&script])),
        Ok(CapOutcome::Value(CapValue::I64(0)))
    );

    // The file changing is the whole reason to pin it.
    std::fs::write(&script, "#!/bin/sh\nexit 1\n").unwrap();
    let err = call_settled(&mut broker, &request(0, "process.exec", &[&script])).unwrap_err();
    assert!(err.message.contains("but the grant pins"), "{err}");

    std::fs::remove_file(&script).ok();
}

#[cfg(unix)]
#[test]
fn a_pin_is_checked_on_every_call() {
    // A check that ran earlier tells you what was true earlier.
    let _serialized = scripts_lock();
    let Some(script) = script("recheck.sh", "#!/bin/sh\nexit 0\n") else {
        return;
    };
    let digest = digest_of(&script);
    let mut broker = Broker::new(vec![pinned(
        "process.exec",
        CapKind::Exec,
        &script,
        &digest,
    )]);

    assert!(broker.call(&request(0, "process.exec", &[&script])).is_ok());
    std::fs::write(&script, "#!/bin/sh\nexit 0\n# changed\n").unwrap();
    assert!(
        broker
            .call(&request(0, "process.exec", &[&script]))
            .is_err()
    );

    std::fs::remove_file(&script).ok();
}

// ---- argument vectors ----

/// A grant with `args [...]`.
fn with_args(name: &str, kind: CapKind, constraint: &str, args: &[&str]) -> CapGrant {
    CapGrant {
        args: args.iter().map(|a| (*a).to_string()).collect(),
        ..grant(name, kind, constraint)
    }
}

/// A call that passes an argument vector.
fn exec_request(path: &str, args: &[&str]) -> CapRequest {
    let mut request = request(0, "process.exec", &[path]);
    request.args.push(CapValue::List(
        args.iter().map(|a| (*a).to_string()).collect(),
    ));
    request
}

#[test]
fn arguments_reach_the_program() {
    let path = temp_path("args-out.txt");
    let mut broker = Broker::new(vec![
        with_args("process.exec", CapKind::Exec, "/bin/sh", &["-c"]),
        grant("fs.read", CapKind::Read, &path),
    ]);
    let outcome = broker
        .call(&exec_request(
            "/bin/sh",
            &["-c", &format!("printf ok > {path}")],
        ))
        .expect("the call should be allowed");
    assert_eq!(outcome, CapOutcome::Value(CapValue::I64(0)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "ok");
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_call_has_to_start_with_what_the_grant_pins() {
    let mut broker = Broker::new(vec![with_args(
        "process.exec",
        CapKind::Exec,
        "/bin/echo",
        &["sic:"],
    )]);
    let err = broker
        .call(&exec_request("/bin/echo", &["elsewhere"]))
        .expect_err("a different first argument is a different call");
    assert!(err.message.contains("starting"), "{}", err.message);
}

/// A prefix bounds the start of the vector and nothing after it. That is the
/// claim `docs/design/arguments.md` makes, so it is the claim a test makes.
#[test]
fn what_follows_the_prefix_is_free() {
    let mut broker = Broker::new(vec![with_args(
        "process.exec",
        CapKind::Exec,
        "/bin/echo",
        &["sic:"],
    )]);
    assert!(
        broker
            .call(&exec_request("/bin/echo", &["sic:", "anything at all"]))
            .is_ok()
    );
}

/// Every grant written before arguments existed keeps exactly the authority it
/// had: it may run the file, and may not tell it anything.
#[test]
fn a_grant_that_pins_no_arguments_allows_none() {
    let mut broker = Broker::new(vec![grant("process.exec", CapKind::Exec, "/bin/echo")]);
    let err = broker
        .call(&exec_request("/bin/echo", &["surprise"]))
        .expect_err("an empty prefix is not a wildcard");
    assert!(err.message.contains("no arguments"), "{}", err.message);
    assert!(
        broker
            .call(&request(0, "process.exec", &["/bin/echo"]))
            .is_ok(),
        "leaving the vector off passes an empty one"
    );
}

// ---- reading what a program said ----

fn capture_request(path: &str, args: &[&str]) -> CapRequest {
    let mut request = exec_request(path, args);
    request.name = "process.capture".into();
    request
}

#[test]
fn capture_returns_what_the_program_printed() {
    let mut broker = Broker::new(vec![with_args(
        "process.capture",
        CapKind::Exec,
        "/bin/echo",
        &["sic:"],
    )]);
    let outcome = broker
        .call(&capture_request("/bin/echo", &["sic:", "hello"]))
        .expect("the call should be allowed");
    assert_eq!(
        outcome,
        CapOutcome::Value(CapValue::Str("sic: hello\n".into()))
    );
}

/// What a program printed on its way to failing is not an answer, and the one
/// useful part of the failure is what it said on stderr.
#[test]
fn a_non_zero_exit_is_a_failure_carrying_stderr() {
    let mut broker = Broker::new(vec![with_args(
        "process.capture",
        CapKind::Exec,
        "/bin/sh",
        &["-c"],
    )]);
    let err = broker
        .call(&capture_request(
            "/bin/sh",
            &["-c", "echo trouble >&2; exit 3"],
        ))
        .expect_err("a program that failed produces no value");
    assert!(err.message.contains("exited 3"), "{}", err.message);
    assert!(err.message.contains("trouble"), "{}", err.message);
}

/// An answer that looks whole but is not would parse, validate, and be wrong.
#[test]
fn output_past_the_limit_fails_rather_than_truncates() {
    let mut broker = Broker::new(vec![with_args(
        "process.capture",
        CapKind::Exec,
        "/bin/sh",
        &["-c"],
    )]);
    let script = format!("yes sic | head -c {}", MAX_OUTPUT + 1);
    let err = broker
        .call(&capture_request("/bin/sh", &["-c", &script]))
        .expect_err("more than the limit is a failure");
    assert!(err.message.contains("more than"), "{}", err.message);
}

/// The grant is exec's grant: the same prefix rule decides both.
#[test]
fn capture_obeys_the_pinned_prefix() {
    let mut broker = Broker::new(vec![with_args(
        "process.capture",
        CapKind::Exec,
        "/bin/echo",
        &["sic:"],
    )]);
    assert!(
        broker
            .call(&capture_request("/bin/echo", &["elsewhere"]))
            .is_err()
    );
}

/// Honouring a deadline while draining a pipe needs a reader thread. Until
/// then, refusing beats telling a program its call was bounded when it was not.
#[test]
fn capture_refuses_a_deadline_it_cannot_honour() {
    let mut broker = Broker::new(vec![with_args(
        "process.capture",
        CapKind::Exec,
        "/bin/echo",
        &["sic:"],
    )]);
    let mut request = capture_request("/bin/echo", &["sic:"]);
    request.timeout_ms = 500;
    let err = broker
        .call(&request)
        .expect_err("a deadline nothing enforces is refused");
    assert!(err.message.contains("timeout"), "{}", err.message);
}

// ---- driving an agent CLI: docs/design/driving.md ----

use crate::agent::{
    AgentDriver, Ask, DriverInfo, answer_from, ask_text, begin_marker, check_size, end_marker,
    new_marker_id,
};

/// A driver that answers from a script, so that everything except the pane can
/// be tested.
type Heard = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

#[derive(Debug)]
struct FakeAgent {
    name: String,
    info: DriverInfo,
    said: Vec<String>,
    /// What it was asked, so a test can read the question the broker composed.
    asked: Heard,
}

impl FakeAgent {
    fn new(name: &str, said: &[&str]) -> FakeAgent {
        FakeAgent::listening(name, said, Heard::default())
    }

    fn listening(name: &str, said: &[&str], asked: Heard) -> FakeAgent {
        FakeAgent {
            name: name.into(),
            info: DriverInfo {
                driver: format!("fake:{name}"),
                command: format!("/nowhere/{name}"),
                agent: format!("{name} 0.0.0"),
                multiplexer: "none".into(),
                instructions: Vec::new(),
            },
            said: said.iter().map(|s| (*s).to_string()).collect(),
            asked,
        }
    }
}

impl AgentDriver for FakeAgent {
    fn agent_name(&self) -> &str {
        &self.name
    }
    fn info(&self) -> &DriverInfo {
        &self.info
    }
    fn ask(&mut self, ask: Ask<'_>) -> Result<String, CapError> {
        self.asked.borrow_mut().push(format!(
            "t{}c{} {}",
            ask.thread.task, ask.thread.conversation, ask.prompt
        ));
        match self.said.is_empty() {
            true => Err(CapError::new("the agent has nothing left to say")),
            false => Ok(self.said.remove(0)),
        }
    }

    fn finish(&mut self, _waiting: bool) {}
}

fn invoke(prompt: &str) -> CapRequest {
    request(0, "llm.invoke", &[prompt])
}

fn invoke_shaped(prompt: &str, shape: &str) -> CapRequest {
    request(0, "llm.invoke", &[prompt, shape])
}

fn a_session() -> crate::tmux::Session {
    crate::tmux::Session {
        run: "0123456789abcdef".into(),
        continuing: false,
        state: None,
    }
}

#[test]
fn a_broker_with_no_driver_still_defers() {
    let mut broker = Broker::new(vec![grant("llm.invoke", CapKind::Invoke, "claude")]);
    let answer = broker.call(&invoke("why is it slow?"));
    assert_eq!(
        answer,
        Ok(CapOutcome::Deferred {
            question: "[claude] why is it slow?".into()
        })
    );
}

#[test]
fn a_driver_answers_within_the_call() {
    let mut broker = Broker::with_driver(
        vec![grant("llm.invoke", CapKind::Invoke, "claude")],
        Box::new(FakeAgent::new("claude", &["{\"cause\":\"disk full\"}"])),
    );
    let answer = broker.call(&invoke("why is it slow?"));
    assert_eq!(
        answer,
        Ok(CapOutcome::Value(CapValue::Str(
            "{\"cause\":\"disk full\"}".into()
        )))
    );
}

#[test]
fn a_grant_naming_another_agent_is_refused() {
    // Answering with whatever happened to be installed would leave the
    // manifest recording a claim that was not true.
    let mut broker = Broker::with_driver(
        vec![grant("llm.invoke", CapKind::Invoke, "gpt-5")],
        Box::new(FakeAgent::new("claude", &["anything"])),
    );
    let error = broker
        .call(&invoke("hello"))
        .expect_err("the grant disagrees");
    assert!(error.message.contains("gpt-5"), "{}", error.message);
    assert!(error.message.contains("claude"), "{}", error.message);
}

#[test]
fn a_driven_call_still_refuses_a_deadline() {
    let mut broker = Broker::with_driver(
        vec![grant("llm.invoke", CapKind::Invoke, "claude")],
        Box::new(FakeAgent::new("claude", &["anything"])),
    );
    let mut request = invoke("hello");
    request.timeout_ms = 10;
    let error = broker.call(&request).expect_err("a deadline is refused");
    assert!(error.message.contains("cannot honour a timeout"));
}

#[test]
fn the_instructions_never_contain_the_marker_they_describe() {
    // The whole protocol rests on this. Whatever is typed into a pane is echoed
    // back into it, so instructions holding the literal marker would put a
    // complete-looking answer on screen before the agent had answered anything.
    let id = "9f2c1a4b";
    let text = ask_text("why is it slow?", id, true);
    assert!(!text.contains(&begin_marker(id)), "{text}");
    assert!(!text.contains(&end_marker(id)), "{text}");
    // And it is not because the id is missing: the pieces are all there.
    assert!(text.contains(id));
    assert!(answer_from(&text, id, true).is_none());
}

#[test]
fn an_answer_is_what_lies_between_the_markers() {
    let id = "abc123";
    let screen = format!(
        "some banner\n{}\n{{\"cause\": \"disk full\"}}\n{}\n",
        begin_marker(id),
        end_marker(id)
    );
    assert_eq!(
        answer_from(&screen, id, false).as_deref(),
        Some("{\"cause\": \"disk full\"}")
    );
}

#[test]
fn an_unfinished_answer_is_not_one() {
    let id = "abc123";
    let screen = format!("{}\n{{\"cause\": \"disk", begin_marker(id));
    assert_eq!(answer_from(&screen, id, false), None);
    assert_eq!(answer_from("nothing at all", id, false), None);
}

#[test]
fn the_last_answer_on_the_screen_is_the_one_that_counts() {
    // An agent that answered twice - a retry inside its own conversation - has
    // the answer that counts last.
    let id = "abc123";
    let screen = format!(
        "{b}\nfirst\n{e}\nthinking again\n{b}\nsecond\n{e}\n",
        b = begin_marker(id),
        e = end_marker(id)
    );
    assert_eq!(answer_from(&screen, id, false).as_deref(), Some("second"));
}

#[test]
fn a_marker_from_another_call_does_not_end_this_one() {
    let screen = format!(
        "{}\nan older answer\n{}\n",
        begin_marker("old00000"),
        end_marker("old00000")
    );
    assert_eq!(answer_from(&screen, "new00000", false), None);
}

#[test]
fn the_interface_is_stripped_from_the_answer() {
    // What `capture-pane` gives back: bullets on the agent's lines, the input
    // box drawn underneath, and every line padded out to the pane's width.
    let id = "abc123";
    let screen = format!(
        "⏺ Reading the logs…                    \n\
         ⏺ {b}                                  \n\
         \x20 {{                                \n\
         \x20   \"cause\": \"disk full\"         \n\
         \x20 }}                                \n\
         \x20 {e}                               \n\
         ╭──────────────────────────────────────╮\n\
         │ >                                    │\n\
         ╰──────────────────────────────────────╯\n",
        b = begin_marker(id),
        e = end_marker(id)
    );
    assert_eq!(
        answer_from(&screen, id, false).as_deref(),
        Some("{\n\"cause\": \"disk full\"\n}")
    );
}

#[test]
fn two_ids_are_not_the_same_id() {
    assert_ne!(new_marker_id(), new_marker_id());
}

#[test]
fn a_pane_too_large_to_be_an_answer_is_refused() {
    assert!(check_size("small").is_ok());
    let huge = "x".repeat(crate::agent::MAX_ANSWER + 1);
    let error = check_size(&huge).expect_err("over the limit");
    assert!(error.message.contains("over the"), "{}", error.message);
}

#[test]
fn a_driver_spec_says_what_drives_what() {
    // These fail on the spec, before anything is looked for on the machine.
    let error = crate::TmuxDriver::open("claude", a_session()).expect_err("no multiplexer");
    assert!(error.message.contains("tmux:claude"), "{}", error.message);

    let error = crate::TmuxDriver::open("screen:claude", a_session()).expect_err("not tmux");
    assert!(error.message.contains("only one"), "{}", error.message);

    let error = crate::TmuxDriver::open("tmux:", a_session()).expect_err("no agent");
    assert!(
        error.message.contains("names no agent"),
        "{}",
        error.message
    );
}

/// An `agent` declares the shape of its answer once. Whoever answers has to be
/// told, or the run fails at the validation for the wrong reason.
#[test]
fn the_shape_of_the_answer_is_part_of_what_is_asked() {
    let heard = Heard::default();
    let mut broker = Broker::with_driver(
        vec![grant("llm.invoke", CapKind::Invoke, "claude")],
        Box::new(FakeAgent::listening(
            "claude",
            &["{\"title\": \"disk\"}"],
            heard.clone(),
        )),
    );
    let answered = broker.call(&invoke_shaped(
        "the deploy job is stuck",
        "{\"title\": string}",
    ));
    assert!(answered.is_ok(), "{answered:?}");

    let heard = heard.borrow();
    let asked = heard.first().expect("the driver was asked something");
    assert!(asked.contains("the deploy job is stuck"), "{asked}");
    assert!(asked.contains("{\"title\": string}"), "{asked}");
}

/// A person answering a deferred call is told exactly what a model would have
/// been told. They are answering the same question.
#[test]
fn a_deferred_call_carries_the_shape_too() {
    let mut broker = Broker::new(vec![grant("llm.invoke", CapKind::Invoke, "claude")]);
    let CapOutcome::Deferred { question } = broker
        .call(&invoke_shaped("why is it slow?", "{\"cause\": string}"))
        .expect("deferred")
    else {
        panic!("a model call with no driver defers");
    };
    assert!(question.contains("why is it slow?"), "{question}");
    assert!(question.contains("{\"cause\": string}"), "{question}");
    assert!(question.contains("Reply with JSON"), "{question}");
}

/// A call that wants prose leaves the shape off, and is not decorated with an
/// empty one.
#[test]
fn no_shape_means_nothing_is_added() {
    let mut broker = Broker::new(vec![grant("llm.invoke", CapKind::Invoke, "claude")]);
    let answered = broker.call(&invoke_shaped("summarize this", ""));
    assert_eq!(
        answered,
        Ok(CapOutcome::Deferred {
            question: "[claude] summarize this".into()
        })
    );
}

/// The driver is told which conversation a call belongs to, because the pair -
/// the caller and the task - is what identifies one.
#[test]
fn a_call_says_which_conversation_it_belongs_to() {
    let heard = Heard::default();
    let mut broker = Broker::with_driver(
        vec![grant("llm.invoke", CapKind::Invoke, "claude")],
        Box::new(FakeAgent::listening("claude", &["ok", "ok"], heard.clone())),
    );

    let mut first = invoke("what is wrong?");
    first.task = 2;
    first.conversation = 1;
    broker.call(&first).expect("answered");

    // The default is a fresh conversation, which is what an agent without
    // `memory: task` means.
    broker.call(&invoke("and this?")).expect("answered");

    let heard = heard.borrow();
    assert!(heard[0].starts_with("t2c1 "), "{}", heard[0]);
    assert!(heard[1].starts_with("t0c0 "), "{}", heard[1]);
}

/// What a run had open is read back, so a pane that was closed can be told from
/// one that was never made.
#[test]
fn the_conversations_a_run_left_open_are_read_back() {
    let path = std::path::PathBuf::from(temp_path("conversations"));
    std::fs::write(&path, "1 0\n1 2\n\nnonsense\n2 0\n").expect("writable");
    assert_eq!(crate::tmux::read_state(&path), vec![(1, 0), (1, 2), (2, 0)]);
    // A run that never opened one has nothing to lose.
    assert!(crate::tmux::read_state(std::path::Path::new("/nowhere/at/all")).is_empty());
    std::fs::remove_file(&path).ok();
}

/// The interface draws an answer at the width it has, so a long JSON string
/// comes back with a line break inside it - where a literal newline is not even
/// legal. Joining repairs the wrap exactly, because JSON needs whitespace
/// nowhere.
#[test]
fn a_wrapped_json_answer_is_put_back_together() {
    let id = "abc123";
    let screen = format!(
        "{b}\n{{\"reason\": \"a line long enough that the interface broke it in\nhalf\"}}\n{e}\n",
        b = begin_marker(id),
        e = end_marker(id)
    );
    assert_eq!(
        answer_from(&screen, id, true).as_deref(),
        Some("{\"reason\": \"a line long enough that the interface broke it inhalf\"}")
    );

    // Prose has no such property, so its line breaks are left alone - the
    // interface's among them.
    assert_eq!(
        answer_from(&screen, id, false).as_deref(),
        Some("{\"reason\": \"a line long enough that the interface broke it in\nhalf\"}")
    );
}

/// An answer asked for as JSON is asked for on one line, so that there is less
/// for the interface to break.
#[test]
fn json_is_asked_for_on_one_line() {
    assert!(ask_text("q", "abc123", true).contains("single line"));
    assert!(!ask_text("q", "abc123", false).contains("single line"));
}

// ---- what the agent may do: docs/design/authority.md ----

use sic_core::{Authority, Reach, Rule, authority_of, reach_of};

fn rules(grant: &CapGrant) -> Vec<String> {
    match reach_of(grant) {
        Reach::Translated(rules) => rules.iter().map(|r| r.to_string()).collect(),
        other => panic!("expected a translation, got {other:?}"),
    }
}

/// A path scope is a thing a permission system can hold, so these become rules
/// in the agent's own configuration rather than calls back into the broker.
#[test]
fn a_path_grant_becomes_a_rule_the_agent_enforces() {
    assert_eq!(
        rules(&grant("fs.read", CapKind::Read, "./docs/x.txt")),
        vec!["Read(./docs/x.txt)"]
    );
    // Writing a whole file and editing part of one are the same authority here,
    // because `fs.write` replaces the file either way.
    assert_eq!(
        rules(&grant("fs.write", CapKind::Write, "./out.txt")),
        vec!["Write(./out.txt)", "Edit(./out.txt)"]
    );
}

/// The grant that summoned the agent is not authority the agent has.
#[test]
fn the_grant_that_summons_the_agent_is_not_the_agents() {
    let manifest = vec![grant("llm.invoke", CapKind::Invoke, "claude")];
    assert_eq!(reach_of(&manifest[0]), Reach::Summons);
    assert_eq!(authority_of(&manifest), Ok(Authority::default()));
}

/// A shell rule looks like it fits `process.exec` and does not: a command
/// string can invoke anything it likes, and a digest pin has no equivalent at
/// all. Widening a grant to fit the configuration's vocabulary is the one thing
/// a translation must never do.
#[test]
fn running_a_binary_is_not_running_a_shell_command() {
    assert!(matches!(
        reach_of(&pinned(
            "process.exec",
            CapKind::Exec,
            "/usr/bin/cargo",
            "abc"
        )),
        Reach::Routed(_)
    ));
    assert!(matches!(
        reach_of(&grant("process.capture", CapKind::Exec, "/usr/bin/git")),
        Reach::Routed(_)
    ));
    // Asking a person is not a tool the agent has, and it suspends the run.
    assert!(matches!(
        reach_of(&grant("human.approve", CapKind::Invoke, "deploying")),
        Reach::Routed(_)
    ));
}

/// What the agent's permission system cannot hold, it does not get: the tool is
/// denied and the capability arrives at the broker instead, named as a tool the
/// agent may call.
#[test]
fn what_cannot_be_translated_is_offered_through_the_broker() {
    let manifest = vec![
        grant("llm.invoke", CapKind::Invoke, "claude"),
        grant("fs.read", CapKind::Read, "./docs"),
        pinned("process.exec", CapKind::Exec, "/usr/bin/cargo", "abc"),
    ];
    let authority = authority_of(&manifest).expect("all of it reaches the agent somehow");
    let rules: Vec<String> = authority.allowed.iter().map(Rule::to_string).collect();
    assert_eq!(rules, ["Read(./docs)", "mcp__sic__process_exec"]);
    assert_eq!(authority.routed, ["process.exec"]);
}

/// A manifest that cannot be enforced is worse than none once `sic plan` has
/// printed it, so the run does not start - and the refusal names the grant.
#[test]
fn a_grant_nothing_can_enforce_stops_the_run_before_it_starts() {
    // A capability the compiler could know and this broker does not.
    let manifest = vec![grant("net.fetch", CapKind::Read, "https://example.com")];
    let refused = authority_of(&manifest).expect_err("nothing can enforce it");
    assert!(refused.grant.contains("net.fetch"), "{refused}");
    assert!(
        refused
            .to_string()
            .contains("neither translated nor routed"),
        "{refused}"
    );
}

/// Everything the agent may do comes from the manifest, in the order the
/// manifest names it.
#[test]
fn the_agents_authority_is_the_programs_manifest() {
    let manifest = vec![
        grant("fs.read", CapKind::Read, "./docs"),
        grant("llm.invoke", CapKind::Invoke, "claude"),
        grant("fs.write", CapKind::Write, "./out"),
    ];
    let authority = authority_of(&manifest).expect("all of it translates");
    let names: Vec<String> = authority.allowed.iter().map(Rule::to_string).collect();
    assert_eq!(names, ["Read(./docs)", "Write(./out)", "Edit(./out)"]);
    assert!(authority.routed.is_empty());
}

/// A tool named nowhere has to be denied without prompting, because the pane
/// has nobody watching it: any mode that can prompt would hang.
#[test]
fn the_agent_is_started_with_an_allowlist_and_no_way_to_ask() {
    let manifest = vec![
        grant("llm.invoke", CapKind::Invoke, "claude"),
        grant("fs.read", CapKind::Read, "./docs"),
    ];
    let args = authority_of(&manifest).expect("it translates").arguments();
    let line = args.join(" ");

    assert!(line.contains("--permission-mode dontAsk"), "{line}");
    assert!(line.contains("--allowedTools Read(./docs)"), "{line}");
    // The two tools that reach the network without going through the Bash
    // sandbox. A deny rule is the only part of this configuration that another
    // settings file cannot widen.
    assert!(
        line.contains("--disallowedTools WebFetch WebSearch"),
        "{line}"
    );
}

/// A program that grants nothing beyond summoning the agent gives the agent
/// nothing - not an unbounded session.
#[test]
fn a_manifest_that_grants_nothing_allows_nothing() {
    let manifest = vec![grant("llm.invoke", CapKind::Invoke, "claude")];
    let args = authority_of(&manifest).expect("it translates").arguments();
    assert!(!args.iter().any(|a| a == "--allowedTools"), "{args:?}");
    assert!(args.iter().any(|a| a == "dontAsk"), "{args:?}");
}

/// The whole of routing, with no agent in it: a call arrives on the socket, is
/// authorized against the program's manifest, is performed by the same code
/// that performs a call from the VM, and is recorded.
///
/// The pin is the point. `process.exec ... sha256` hashes the file on every
/// call, and no permission setting in any agent expresses that - which is why
/// this capability is routed rather than translated.
#[test]
fn a_routed_call_is_the_same_call() {
    let _lock = scripts_lock();
    let Some(path) = script("routed", "#!/bin/sh\nexit 0\n") else {
        return;
    };
    let digest = digest_of(&path);
    let manifest = vec![pinned("process.exec", CapKind::Exec, &path, &digest)];

    let socket = std::path::PathBuf::from(temp_path("route.sock"));
    let mut route = crate::route::Route::open(socket.clone(), manifest).expect("a socket");

    // What the agent is told it may call.
    let offered = crate::route::offered(route_manifest(&path, &digest).as_slice());
    assert_eq!(offered[0].tool_name(), "process_exec");

    let request = CapRequest {
        index: 0,
        name: "process.exec".into(),
        args: vec![CapValue::Str(path.clone()), CapValue::List(Vec::new())],
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        tools_left: 0,
        answer_ms: 0,
    };

    // The caller is on the other side of the socket, so it has to be answered
    // while this side is looking - which is what the driver's loop does.
    let asking = std::thread::spawn({
        let socket = socket.clone();
        move || crate::route::ask(&socket, &request)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !asking.is_finished() && std::time::Instant::now() < deadline {
        route.serve_pending();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let bytes = asking
        .join()
        .expect("the asker finished")
        .expect("answered");
    assert_eq!(
        crate::route::answer(&bytes),
        Ok(CapOutcome::Value(CapValue::I64(0)))
    );

    // And it is on the record, as digests.
    let used = route.take_tool_uses();
    assert_eq!(used.len(), 1);
    match &used[0] {
        sic_core::AgentAction::Capability { cap, outcome, .. } => {
            assert_eq!(cap, "process.exec");
            assert!(outcome.is_ok());
        }
        other => panic!("a routed call is a capability call: {other:?}"),
    }
    assert!(route.take_tool_uses().is_empty(), "draining happens once");

    std::fs::remove_file(&path).ok();
}

/// A file whose digest is not the one the grant pins fails the routed call, the
/// same way it fails a call from a program.
#[test]
fn a_routed_call_is_checked_against_the_pin() {
    let _lock = scripts_lock();
    let Some(path) = script("routed-pin", "#!/bin/sh\nexit 0\n") else {
        return;
    };
    let manifest = vec![pinned(
        "process.exec",
        CapKind::Exec,
        &path,
        &"0".repeat(64),
    )];
    let request = CapRequest {
        index: 0,
        name: "process.exec".into(),
        args: vec![CapValue::Str(path.clone()), CapValue::List(Vec::new())],
        task: 0,
        attempt: 1,
        timeout_ms: 0,
        conversation: 0,
        tools_left: 0,
        answer_ms: 0,
    };
    let error = crate::perform(&manifest, &request).expect_err("the pin does not match");
    assert!(error.message.contains("but the grant pins"), "{error}");
    std::fs::remove_file(&path).ok();
}

/// An agent cannot summon another agent: the path its calls arrive on does not
/// reach what answers a model call.
#[test]
fn an_agent_may_not_summon_another_agent() {
    let manifest = vec![grant("llm.invoke", CapKind::Invoke, "claude")];
    let error = crate::perform(&manifest, &invoke("why?")).expect_err("refused");
    assert!(error.message.contains("summon another agent"), "{error}");
}

fn route_manifest(path: &str, digest: &str) -> Vec<CapGrant> {
    vec![pinned("process.exec", CapKind::Exec, path, digest)]
}

/// An agent that joins the marker imperfectly still gets its answer read.
///
/// This is what happened the first time one used a tool and then answered: it
/// printed two angle brackets where the instructions asked for three. The
/// answer was right; the run waited half an hour for a marker three characters
/// away from the one it wanted.
#[test]
fn a_marker_joined_imperfectly_is_still_a_marker() {
    let id = "ebcc06e5";
    let screen =
        format!("some banner\n<<SIC-BEGIN-{id}>>\n{{\"said\": \"it worked\"}}\n<<SIC-END-{id}>>\n");
    assert_eq!(
        answer_from(&screen, id, true).as_deref(),
        Some("{\"said\": \"it worked\"}")
    );

    // And the property the whole protocol rests on is untouched: the split
    // falls between the `S` and the `IC`, so nothing in the instructions is a
    // marker however it is read.
    let text = ask_text("a question", id, true);
    assert!(!text.contains(&format!("SIC-BEGIN-{id}")), "{text}");
    assert!(!text.contains(&format!("SIC-END-{id}")), "{text}");
    assert!(answer_from(&text, id, true).is_none());
}

/// A shell is refused whatever the rules say.
///
/// `dontAsk` always permits a fixed set of read-only commands - `cat` among
/// them - and the set is not configurable, so a rule scoping `Read` to a
/// directory is not a bound on reading. The hook runs before the rules and can
/// refuse what they would have allowed, which is how reading gets bounded at
/// all.
#[test]
fn no_grant_names_a_shell() {
    let socket = std::path::PathBuf::from(temp_path("tool.sock"));
    let manifest = vec![grant("fs.read", CapKind::Read, "./docs")];
    let mut route = crate::route::Route::open(socket.clone(), manifest).expect("a socket");
    // What the manifest accounts for. Everything else is refused, which is the
    // whole tool surface rather than a list of bad tools.
    route.names(vec!["Read".to_string()]);

    for (tool, refused) in [
        ("Bash", true),
        ("PowerShell", true),
        // Never named, and it ran anyway when the rules were the only thing
        // deciding. Now it does not.
        ("ToolSearch", true),
        ("WebFetch", true),
        ("Read", false),
    ] {
        let asking = std::thread::spawn({
            let socket = socket.clone();
            let tool = tool.to_string();
            move || crate::route::may_use(&socket, &tool, "{\"command\":\"cat /etc/shadow\"}")
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !asking.is_finished() && std::time::Instant::now() < deadline {
            route.serve_pending();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let decision = asking
            .join()
            .expect("the asker finished")
            .expect("answered");
        assert_eq!(decision.is_some(), refused, "{tool}");
        // A shell gets its own sentence, because there is somewhere else for it
        // to go. Everything else is told what is actually true of it.
        match (tool, decision) {
            ("Bash" | "PowerShell", Some(reason)) => {
                assert!(reason.contains("no grant names a shell"), "{reason}")
            }
            (_, Some(reason)) => assert!(
                reason.contains("does not account for") && reason.contains(tool),
                "{reason}"
            ),
            (_, None) => {}
        }
    }

    // Every one of them is on the record, refused or not: what the agent
    // reached for is the part the journal was missing.
    let used = route.take_tool_uses();
    assert_eq!(used.len(), 5);
    // A tool of the agent's own is not recorded as a capability: the manifest
    // does not name `Bash`, and the journal does not borrow a word that means
    // something else here.
    for (i, allowed_expected) in [(0, false), (4, true)] {
        match &used[i] {
            sic_core::AgentAction::Tool { allowed, .. } => {
                assert_eq!(*allowed, allowed_expected)
            }
            other => panic!("the agent's own tool is not a capability: {other:?}"),
        }
    }
}

/// A tool allowance is a bound, not a note: the call that used it up is refused
/// the next one, and told which of the two reasons it was.
#[test]
fn an_answer_that_used_its_allowance_gets_no_more_tools() {
    let socket = std::path::PathBuf::from(temp_path("allowance.sock"));
    let manifest = vec![grant("fs.read", CapKind::Read, "./docs")];
    let mut route = crate::route::Route::open(socket.clone(), manifest).expect("a socket");
    route.names(vec!["Read".to_string()]);
    route.allow(2);

    let mut reasons = Vec::new();
    for _ in 0..3 {
        let asking = std::thread::spawn({
            let socket = socket.clone();
            move || crate::route::may_use(&socket, "Read", "{}")
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !asking.is_finished() && std::time::Instant::now() < deadline {
            route.serve_pending();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        reasons.push(asking.join().expect("finished").expect("answered"));
    }

    assert_eq!(reasons[0], None);
    assert_eq!(reasons[1], None);
    let spent = reasons[2].as_deref().expect("the third is refused");
    assert!(spent.contains("every tool it was allowed"), "{spent}");
    // And not for the other reason: which one it was is the point.
    assert!(!spent.contains("shell"), "{spent}");
}

/// A grant names a path, and a symbolic link at that path is followed.
///
/// This is the decision rather than an oversight, and it is written down in
/// `docs/design/capabilities.md` so that a plan's reader knows what they are
/// approving. Refusing links was tried and is not available: `/bin` is a link
/// to `/usr/bin` on any system that merged them, so a rule refusing one
/// anywhere along a path refuses `/bin/sh`, and a rule with an exception for
/// the links a distribution ships is not a rule.
///
/// What a plan promises is "this program may open this path". The answer to
/// "these bytes" is a pin, which `process.exec` has and `fs.read` does not.
#[test]
fn a_grant_follows_a_symbolic_link_deliberately() {
    let target = temp_path("linked-target.txt");
    let link = temp_path("the-link.txt");
    std::fs::write(&target, "what it points at").expect("writable");
    std::fs::remove_file(&link).ok();
    if std::os::unix::fs::symlink(&target, &link).is_err() {
        // A filesystem without links has nothing to say here.
        return;
    }

    let mut broker = Broker::new(vec![grant("fs.read", CapKind::Read, &link)]);
    assert_eq!(
        broker.call(&request(0, "fs.read", &[&link])),
        Ok(CapOutcome::Value(CapValue::Str("what it points at".into())))
    );

    // And the grant is still exactly one path: the link does not widen it to
    // whatever else is beside its target.
    let error = broker
        .call(&request(0, "fs.read", &[&target]))
        .expect_err("the grant names the link, not the file behind it");
    assert!(
        error.message.contains("may only be used with"),
        "{}",
        error.message
    );

    std::fs::remove_file(&link).ok();
    std::fs::remove_file(&target).ok();
}

/// The record has to be able to say whether the agent was told the same thing.
///
/// Two runs of the same program that got different answers should be
/// distinguishable from the record, and an instruction file in the working
/// directory is one of the three things that decided the answer - alongside the
/// prompt, which the journal digests, and the output type, which the program
/// declares. It was the one with no trace at all.
#[test]
fn what_the_agent_was_told_is_digested() {
    let dir = std::path::PathBuf::from(temp_path("told"));
    std::fs::create_dir_all(&dir).expect("writable");
    std::fs::write(dir.join("AGENTS.md"), "answer in French").expect("writable");

    let before = crate::agent::instructions_now(&dir, None);
    let agents = before
        .iter()
        .find(|i| i.path.ends_with("AGENTS.md"))
        .expect("it was looked for");
    assert!(agents.digest.is_some(), "it is there, so it has a digest");

    // A file that is not there is recorded as not there, which an empty list
    // could not say.
    let claude = before
        .iter()
        .find(|i| i.path.ends_with("CLAUDE.md"))
        .expect("it was looked for");
    assert!(claude.digest.is_none());

    // Change what the agent is told, and the record changes with it.
    std::fs::write(dir.join("AGENTS.md"), "answer in German").expect("writable");
    let after = crate::agent::instructions_now(&dir, None);
    assert_ne!(before, after);

    std::fs::remove_dir_all(&dir).ok();
}
