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
    AgentDriver, DriverInfo, answer_from, ask_text, begin_marker, check_size, end_marker,
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
    fn ask(&mut self, prompt: &str) -> Result<String, CapError> {
        self.asked.borrow_mut().push(prompt.to_string());
        match self.said.is_empty() {
            true => Err(CapError::new("the agent has nothing left to say")),
            false => Ok(self.said.remove(0)),
        }
    }
}

fn invoke(prompt: &str) -> CapRequest {
    request(0, "llm.invoke", &[prompt])
}

fn invoke_shaped(prompt: &str, shape: &str) -> CapRequest {
    request(0, "llm.invoke", &[prompt, shape])
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
    let text = ask_text("why is it slow?", id);
    assert!(!text.contains(&begin_marker(id)), "{text}");
    assert!(!text.contains(&end_marker(id)), "{text}");
    // And it is not because the id is missing: the pieces are all there.
    assert!(text.contains(id));
    assert!(answer_from(&text, id).is_none());
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
        answer_from(&screen, id).as_deref(),
        Some("{\"cause\": \"disk full\"}")
    );
}

#[test]
fn an_unfinished_answer_is_not_one() {
    let id = "abc123";
    let screen = format!("{}\n{{\"cause\": \"disk", begin_marker(id));
    assert_eq!(answer_from(&screen, id), None);
    assert_eq!(answer_from("nothing at all", id), None);
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
    assert_eq!(answer_from(&screen, id).as_deref(), Some("second"));
}

#[test]
fn a_marker_from_another_call_does_not_end_this_one() {
    let screen = format!(
        "{}\nan older answer\n{}\n",
        begin_marker("old00000"),
        end_marker("old00000")
    );
    assert_eq!(answer_from(&screen, "new00000"), None);
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
        answer_from(&screen, id).as_deref(),
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
    let error = crate::TmuxDriver::open("claude").expect_err("no multiplexer");
    assert!(error.message.contains("tmux:claude"), "{}", error.message);

    let error = crate::TmuxDriver::open("screen:claude").expect_err("not tmux");
    assert!(error.message.contains("only one"), "{}", error.message);

    let error = crate::TmuxDriver::open("tmux:").expect_err("no agent");
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
