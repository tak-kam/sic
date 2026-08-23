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
