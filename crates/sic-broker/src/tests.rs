use sic_core::{CapGrant, CapKind, CapOutcome, CapRequest, CapValue};

use super::*;

fn grant(name: &str, kind: CapKind, constraint: &str) -> CapGrant {
    CapGrant {
        name: name.into(),
        kind,
        constraint: constraint.into(),
    }
}

fn request(index: u32, name: &str, args: &[&str]) -> CapRequest {
    CapRequest {
        index,
        name: name.into(),
        args: args.iter().map(|a| CapValue::Str((*a).into())).collect(),
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
