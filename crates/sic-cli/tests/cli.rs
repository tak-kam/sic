//! End-to-end tests that run the built binary.

use std::process::Command;

fn sic(args: &[&str]) -> (String, String, i32) {
    sic_in(repo_root(), args)
}

/// Runs the binary with a working directory, for programs whose capability
/// grants name relative paths.
fn sic_in(dir: std::path::PathBuf, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_sic"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run sic");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Writes a source file to a temporary path (no tempfile crate).
fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn parses_the_milestone_example() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/milestone.sic");
    let (stdout, stderr, code) = sic(&["parse", path]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        stdout,
        "\
(module
  (fn main
    (block
      (let x 10)
      (let y (+ x 20))
      (return y))))
"
    );
}

#[test]
fn reports_errors_with_source_location_and_exit_code_1() {
    let path = write_temp("bad.sic", "fn main() {\n    let y = x + ;\n}\n");
    let (_, stderr, code) = sic(&["parse", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0204"), "{stderr}");
    assert!(stderr.contains(":2:17"), "{stderr}");
    assert!(stderr.contains("aborting due to 1 error"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn rejects_bom() {
    let path = write_temp("bom.sic", "\u{FEFF}fn main() {}\n");
    let (_, stderr, code) = sic(&["parse", path.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert!(stderr.contains("BOM"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn missing_file_is_a_usage_error() {
    let (_, stderr, code) = sic(&["parse", "/nonexistent/whatever.sic"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("cannot read"), "{stderr}");
}

#[test]
fn no_args_prints_usage() {
    let (_, stderr, code) = sic(&[]);
    assert_eq!(code, 2);
    assert!(stderr.contains("Usage:"), "{stderr}");
}

// ---- the whole pipeline: source -> bytecode -> verifier -> VM ----

fn example(name: &str) -> String {
    format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn runs_the_milestone_example() {
    let (stdout, stderr, code) = sic(&["run", &example("milestone.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "30\n");
}

#[test]
fn runs_recursion_and_short_circuit_examples() {
    let (stdout, stderr, code) = sic(&["run", &example("factorial.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "3628800\n");

    let (stdout, stderr, code) = sic(&["run", &example("branching.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "1\n");
}

#[test]
fn a_type_error_stops_the_run() {
    let path = write_temp("type-error.sic", "fn main() {\n    return 1 + true;\n}\n");
    let (stdout, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("E0303"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn a_runtime_failure_names_the_source_position() {
    let path = write_temp(
        "div0.sic",
        "fn main() {\n    let n = 0;\n    return 10 / n;\n}\n",
    );
    let (_, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("division by zero"), "{stderr}");
    // The debug section maps the failing instruction back to the source.
    assert!(stderr.contains(":3:12"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn running_a_module_without_main_is_an_error() {
    let path = write_temp("nomain.sic", "fn helper() -> Int { return 1; }\n");
    let (_, stderr, code) = sic(&["run", path.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("no `main`"), "{stderr}");
    std::fs::remove_file(path).ok();
}

#[test]
fn compile_then_verify_then_disassemble() {
    let src = write_temp("pipeline.sic", "fn main() {\n    return 6 * 7;\n}\n");
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();

    let (stdout, stderr, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.starts_with("wrote "), "{stdout}");

    let (stdout, stderr, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("1 function(s) verified"), "{stdout}");
    assert!(stdout.contains("required capabilities:"), "{stdout}");

    let (stdout, _, code) = sic(&["disasm", &out_str]);
    assert_eq!(code, 0);
    assert!(stdout.contains("MUL_I64"), "{stdout}");
    assert!(stdout.contains("RETURN"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn verify_rejects_a_corrupted_file() {
    let src = write_temp("corrupt.sic", "fn main() {\n    return 1;\n}\n");
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();
    let (_, _, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0);

    let mut bytes = std::fs::read(&out).unwrap();
    bytes[0] = b'X'; // break the magic
    std::fs::write(&out, &bytes).unwrap();

    let (_, stderr, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 1);
    assert!(stderr.contains("magic"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn hir_prints_the_intermediate_representation() {
    let (stdout, stderr, code) = sic(&["hir", &example("milestone.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("fn main/0:"), "{stdout}");
    assert!(stdout.contains("bb0:"), "{stdout}");
}

// ---- capabilities ----

#[test]
fn a_granted_capability_is_performed_by_the_broker() {
    let data = write_temp("cap-data.txt", "hello from a file");
    let data = data.to_str().unwrap().to_string();
    let src = write_temp(
        "cap-read.sic",
        &format!(
            "allow {{ fs.read {data:?}; }}\nfn main() -> String {{ return fs.read({data:?}); }}\n"
        ),
    );

    let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "\"hello from a file\"\n");

    std::fs::remove_file(&data).ok();
    std::fs::remove_file(src).ok();
}

#[test]
fn calling_a_capability_without_a_grant_does_not_compile() {
    let src = write_temp(
        "cap-nogrant.sic",
        "fn main() -> String { return fs.read(\"./x.txt\"); }\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0320"), "{stderr}");
    // The fix is in the message.
    assert!(stderr.contains("allow {"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn the_broker_refuses_a_path_the_grant_does_not_cover() {
    // The argument is a runtime value, so this compiles: the grant is what
    // stops it, and the broker is where that decision is made.
    let allowed = write_temp("cap-allowed.txt", "allowed");
    let other = write_temp("cap-other.txt", "other");
    let (allowed, other) = (
        allowed.to_str().unwrap().to_string(),
        other.to_str().unwrap().to_string(),
    );
    let src = write_temp(
        "cap-wrongpath.sic",
        &format!(
            "allow {{ fs.read {allowed:?}; }}\nfn main() -> String {{ return fs.read({other:?}); }}\n"
        ),
    );

    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1, "stderr: {stderr}");
    assert!(stderr.contains("a capability call failed"), "{stderr}");
    assert!(stderr.contains("may only be used with"), "{stderr}");

    std::fs::remove_file(&allowed).ok();
    std::fs::remove_file(&other).ok();
    std::fs::remove_file(src).ok();
}

#[test]
fn verify_reports_the_manifest_without_running_anything() {
    let src = write_temp(
        "cap-manifest.sic",
        "allow { process.exec \"/usr/bin/true\"; }\nfn main() -> Int { return process.exec(\"/usr/bin/true\"); }\n",
    );
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();
    let (_, stderr, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (stdout, _, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("process.exec [exec] \"/usr/bin/true\""),
        "{stdout}"
    );

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn an_unused_grant_is_reported_as_a_warning() {
    let src = write_temp(
        "cap-unused.sic",
        "allow { fs.read \"./x.txt\"; }\nfn main() -> Int { return 1; }\n",
    );
    let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "1\n");
    assert!(stderr.contains("never called"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn the_capability_example_runs_from_the_repository_root() {
    let (stdout, stderr, code) = sic_in(repo_root(), &["run", "examples/read-file.sic"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("hello from a file"), "{stdout}");
}

// ---- the execution journal ----

/// The events a journal file holds, in order.
fn journal_events(path: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).expect("the journal should exist");
    text.lines()
        .map(|line| {
            let key = "\"event\":\"";
            let start = line.find(key).expect("every line names its event") + key.len();
            let end = start + line[start..].find('"').unwrap();
            line[start..end].to_string()
        })
        .collect()
}

#[test]
fn a_run_writes_its_journal() {
    let data = write_temp("journal-data.txt", "contents");
    let data = data.to_str().unwrap().to_string();
    let src = write_temp(
        "journal.sic",
        &format!(
            "allow {{ fs.read {data:?}; }}\nfn main() -> String {{ return fs.read({data:?}); }}\n"
        ),
    );
    let journal = src.with_extension("jsonl");

    let (_, stderr, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    // The run announces its id, so a journal can be tied back to a run.
    assert!(stderr.starts_with("run "), "{stderr}");

    assert_eq!(
        journal_events(&journal),
        vec![
            "run_started",
            "task_started",
            "function_entered",
            "capability_requested",
            "capability_completed",
            "function_exited",
            "task_completed",
            "run_completed",
        ]
    );

    std::fs::remove_file(&data).ok();
    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn the_journal_records_digests_not_values() {
    // Telemetry is an exfiltration path, so neither the path read nor the
    // contents may appear in it.
    let data = write_temp("journal-secret.txt", "s3cret-contents");
    let data = data.to_str().unwrap().to_string();
    let src = write_temp(
        "journal-secret.sic",
        &format!(
            "allow {{ fs.read {data:?}; }}\nfn main() -> String {{ return fs.read({data:?}); }}\n"
        ),
    );
    let journal = src.with_extension("jsonl");

    let (_, stderr, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let text = std::fs::read_to_string(&journal).unwrap();
    assert!(!text.contains("s3cret-contents"), "{text}");
    assert!(text.contains("sha256:"), "{text}");

    std::fs::remove_file(&data).ok();
    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn a_failing_run_is_recorded_as_failed() {
    let src = write_temp(
        "journal-fail.sic",
        "fn main() -> Int {\n  let n = 0;\n  return 1 / n;\n}\n",
    );
    let journal = src.with_extension("jsonl");

    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 1);

    let events = journal_events(&journal);
    assert_eq!(events.last().map(String::as_str), Some("run_failed"));
    let text = std::fs::read_to_string(&journal).unwrap();
    assert!(text.contains("division by zero"), "{text}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn without_the_flag_nothing_is_written() {
    // Recording is opted into. A run that was not asked to keep a journal
    // leaves nothing behind.
    let src = write_temp("journal-none.sic", "fn main() -> Int { return 1; }\n");
    let journal = src.with_extension("jsonl");
    let (_, _, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(!journal.exists());
    std::fs::remove_file(src).ok();
}

#[test]
fn journal_lines_are_valid_json_objects() {
    // A hand-written writer earns a check that its output is well formed.
    let src = write_temp("journal-json.sic", "fn main() -> Int { return 1; }\n");
    let journal = src.with_extension("jsonl");
    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);

    for line in std::fs::read_to_string(&journal).unwrap().lines() {
        assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
        // Quotes come in pairs, which a broken escape would break.
        assert_eq!(line.matches('"').count() % 2, 0, "{line}");
        assert!(line.contains("\"seq\":"), "{line}");
        assert!(line.contains("\"run\":\""), "{line}");
    }

    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

// ---- suspend and resume ----

/// A program that asks a person, and does something different with each answer.
const APPROVAL_SRC: &str = "allow { human.approve \"a test\"; }\n\
fn main() -> Int {\n\
    let ok = human.approve(\"go ahead?\");\n\
    if ok { return 1; }\n\
    return 0;\n\
}\n";

#[test]
fn a_run_that_has_to_wait_is_checkpointed() {
    let src = write_temp("suspend.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");

    let (_, stderr, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    // Waiting is not failing, and a caller has to be able to tell them apart.
    assert_eq!(code, 3, "stderr: {stderr}");
    assert!(stderr.contains("waiting: [a test] go ahead?"), "{stderr}");
    assert!(checkpoint.exists());

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn a_checkpointed_run_continues_where_it_stopped() {
    let src = write_temp("resume.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");

    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    // The answer decides which branch the run takes, which shows it really did
    // continue rather than start again.
    let (stdout, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "true",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "1\n");

    let (stdout, _, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "false",
    ]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn the_journal_is_one_sequence_across_both_processes() {
    let src = write_temp("resume-journal.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");
    let journal = src.with_extension("jsonl");

    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    let (_, _, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "true",
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);

    assert_eq!(
        journal_events(&journal),
        vec![
            "run_started",
            "task_started",
            "function_entered",
            "capability_requested",
            "run_suspended",
            "checkpoint_written",
            "run_resumed",
            "capability_completed",
            "function_exited",
            "task_completed",
            "run_completed",
        ]
    );

    // A resumed run is the same run: one run id, and no sequence number used
    // twice.
    let text = std::fs::read_to_string(&journal).unwrap();
    let seqs: Vec<u64> = text
        .lines()
        .map(|l| {
            let start = l.find("\"seq\":").unwrap() + 6;
            let end = start + l[start..].find(',').unwrap();
            l[start..end].parse().unwrap()
        })
        .collect();
    assert_eq!(seqs, (0..11).collect::<Vec<u64>>());

    let run_ids: std::collections::HashSet<&str> = text
        .lines()
        .map(|l| {
            let start = l.find("\"run\":\"").unwrap() + 7;
            &l[start..start + 32]
        })
        .collect();
    assert_eq!(run_ids.len(), 1);

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn waiting_with_nowhere_to_save_is_an_error() {
    let src = write_temp("suspend-nowhere.sic", APPROVAL_SRC);
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("nowhere to be saved"), "{stderr}");
    assert!(stderr.contains("--checkpoint"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn a_checkpoint_cannot_be_resumed_against_changed_source() {
    let src = write_temp("resume-changed.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");
    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    // Editing the program after it was suspended must not silently continue the
    // old run inside the new code.
    let changed = APPROVAL_SRC.replace("return 1;", "return 42;");
    std::fs::write(&src, changed).unwrap();

    let (_, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "true",
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("different bytecode"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn resume_says_what_it_is_waiting_for_when_given_no_answer() {
    let src = write_temp("resume-noanswer.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");
    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    let (_, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
    ]);
    assert_eq!(code, 2);
    assert!(stderr.contains("waiting: [a test] go ahead?"), "{stderr}");
    assert!(stderr.contains("--value <Bool>"), "{stderr}");

    // And an answer of the wrong shape says what shape it should be.
    let (_, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "42",
    ]);
    assert_eq!(code, 2);
    assert!(stderr.contains("not `true` or `false`"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn a_corrupt_checkpoint_is_refused() {
    let src = write_temp("resume-corrupt.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");
    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    let mut bytes = std::fs::read(&checkpoint).unwrap();
    bytes[0] = b'X';
    std::fs::write(&checkpoint, &bytes).unwrap();

    let (_, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "true",
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("bad magic"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(checkpoint).ok();
}

// ---- tasks, retry and timeout ----

#[test]
fn the_concurrency_example_runs() {
    // It calls /usr/bin/true, so skip where that is not present.
    if !std::path::Path::new("/usr/bin/true").exists() {
        return;
    }
    let (stdout, stderr, code) = sic(&["run", &example("tasks.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "0\n");
}

#[test]
fn two_tasks_wait_at_the_same_time() {
    if !std::path::Path::new("/usr/bin/true").exists() {
        return;
    }
    let journal = write_temp("tasks-journal.txt", "");
    let (_, stderr, code) = sic(&[
        "run",
        &example("tasks.sic"),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    // Both requests are recorded before either answer: while one task waits,
    // the other one runs.
    let events = journal_events(&journal);
    let first_completed = events
        .iter()
        .position(|e| e == "capability_completed")
        .expect("a capability completed");
    let requests_before = events[..first_completed]
        .iter()
        .filter(|e| *e == "capability_requested")
        .count();
    assert_eq!(requests_before, 2, "{events:?}");

    // Events name the task they belong to, and it is no longer always zero.
    let text = std::fs::read_to_string(&journal).unwrap();
    let tasks: std::collections::HashSet<&str> = text
        .lines()
        .map(|l| {
            let start = l.find("\"task\":").unwrap() + 7;
            let end = start + l[start..].find(',').unwrap();
            &l[start..end]
        })
        .collect();
    assert!(tasks.len() >= 3, "{tasks:?}");

    std::fs::remove_file(journal).ok();
}

#[test]
fn awaiting_a_task_twice_fails_at_run_time() {
    // A result is moved out of a task, and the checker cannot see that a local
    // is awaited twice, so this is a run-time failure with a clear message.
    let src = write_temp(
        "await-twice.sic",
        "fn work() -> Int { return 1; }\nfn main() -> Int { let t = spawn work(); return await t + await t; }\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("already been awaited"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn a_policy_is_visible_in_the_bytecode() {
    // `sic plan` will read this without executing anything.
    let src = write_temp(
        "policy.sic",
        "allow { fs.read \"./x.txt\"; }\nfn main() -> String { return fs.read(\"./x.txt\") retry 3 timeout 250; }\n",
    );
    let out = src.with_extension("sicb");
    let out_str = out.to_str().unwrap().to_string();
    let (_, stderr, code) = sic(&["compile", src.to_str().unwrap(), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (stdout, _, code) = sic(&["disasm", &out_str]);
    assert_eq!(code, 0);
    assert!(stdout.contains("retry 3"), "{stdout}");
    assert!(stdout.contains("timeout 250ms"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(out).ok();
}

#[test]
fn a_retried_call_records_every_attempt() {
    let src = write_temp(
        "retry-journal.sic",
        "allow { fs.read \"./definitely-missing.txt\"; }\n\
         fn main() -> String { return fs.read(\"./definitely-missing.txt\") retry 3; }\n",
    );
    let journal = src.with_extension("jsonl");
    let (_, stderr, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "stderr: {stderr}");

    let events = journal_events(&journal);
    assert_eq!(
        events
            .iter()
            .filter(|e| *e == "capability_requested")
            .count(),
        3,
        "{events:?}"
    );
    assert_eq!(
        events.iter().filter(|e| *e == "capability_failed").count(),
        3,
        "{events:?}"
    );

    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn a_policy_on_a_function_call_is_a_compile_error() {
    let src = write_temp(
        "policy-bad.sic",
        "fn work() -> Int { return 1; }\nfn main() -> Int { return work() retry 3; }\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0330"), "{stderr}");
    std::fs::remove_file(src).ok();
}

// ---- records and lists ----

#[test]
fn the_records_example_runs() {
    let (stdout, stderr, code) = sic(&["run", &example("records.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    // 5 (a weight) + 2 (the list) + 9 ("disk full")
    assert_eq!(stdout, "16\n");
}

#[test]
fn a_record_value_prints_its_fields() {
    let src = write_temp(
        "record-print.sic",
        "type P { x: Int, y: Int }\nfn main() -> P { return P { x: 1, y: 2 }; }\n",
    );
    let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "{1, 2}\n");
    std::fs::remove_file(src).ok();
}

#[test]
fn an_index_outside_a_list_names_the_source() {
    let src = write_temp(
        "index-oob.sic",
        "fn main() -> Int {\n    let xs = [1, 2];\n    return xs[5];\n}\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("outside the list"), "{stderr}");
    assert!(stderr.contains(":3:12"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn a_type_containing_itself_does_not_compile() {
    let src = write_temp("recursive.sic", "type Loop { next: Loop }\nfn main() { }\n");
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0340"), "{stderr}");
    assert!(stderr.contains("finite size"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn the_structured_example_parses_and_validates() {
    let (stdout, stderr, code) = sic(&["run", &example("structured.sic")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "\"syslog\"\n");
}

#[test]
fn a_document_that_does_not_fit_fails_at_the_boundary() {
    let src = write_temp(
        "schema.sic",
        "type W { value: Int }\n\
         fn main() -> Int {\n\
             let text = \"{\\\"value\\\": \\\"no\\\"}\";\n\
             let w: W = from_json(text);\n\
             return w.value;\n\
         }\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("does not fit the type"), "{stderr}");
    assert!(stderr.contains("value: expected Int"), "{stderr}");
    std::fs::remove_file(src).ok();
}

#[test]
fn from_json_needs_to_know_its_type() {
    let src = write_temp(
        "from-json-untyped.sic",
        "fn main() { let d = from_json(\"{}\"); }\n",
    );
    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0353"), "{stderr}");
    std::fs::remove_file(src).ok();
}

// ---- agents ----

#[test]
fn an_agent_suspends_at_the_model_and_validates_the_answer() {
    let checkpoint = write_temp("agent.sicc", "");
    let (_, stderr, code) = sic(&[
        "run",
        &example("agent.sic"),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    // Calling a model means TLS, so this broker defers it.
    assert_eq!(code, 3, "stderr: {stderr}");
    assert!(stderr.contains("[claude-opus-4]"), "{stderr}");

    let (stdout, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        &example("agent.sic"),
        "--value",
        r#"{"cause": "disk full", "confidence": 0.9}"#,
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "\"disk full\"\n");

    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn an_answer_that_does_not_fit_fails_the_agent() {
    let checkpoint = write_temp("agent-bad.sicc", "");
    let (_, _, code) = sic(&[
        "run",
        &example("agent.sic"),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    // A model that answers with the wrong shape fails at the boundary.
    let (_, stderr, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        &example("agent.sic"),
        "--value",
        r#"{"cause": "disk full"}"#,
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("does not fit the type"), "{stderr}");
    assert!(stderr.contains("needs a field `confidence`"), "{stderr}");

    std::fs::remove_file(checkpoint).ok();
}

#[test]
fn an_agent_is_a_capability_call_in_the_bytecode() {
    // Nothing below the checker knows what an agent is.
    let out = write_temp("agent.sicb", "");
    let out_str = out.to_str().unwrap().to_string();
    let (_, stderr, code) = sic(&["compile", &example("agent.sic"), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (stdout, _, code) = sic(&["disasm", &out_str]);
    assert_eq!(code, 0);
    assert!(stdout.contains("CALL_CAP"), "{stdout}");
    assert!(stdout.contains("FROM_JSON"), "{stdout}");
    assert!(stdout.contains("llm.invoke"), "{stdout}");

    // And `sic verify` reports the model it may talk to, without running it.
    let (stdout, _, code) = sic(&["verify", &out_str]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("llm.invoke [invoke] \"claude-opus-4\""),
        "{stdout}"
    );

    std::fs::remove_file(out).ok();
}

// ---- exporting ----

#[test]
fn a_journal_exports_to_opentelemetry() {
    let src = write_temp("export.sic", "fn main() -> Int { return 1; }\n");
    let journal = src.with_extension("jsonl");
    let traces = src.with_extension("traces.json");
    let metrics = src.with_extension("metrics.json");

    let (_, stderr, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (_, stderr, code) = sic(&[
        "export",
        journal.to_str().unwrap(),
        "--traces",
        traces.to_str().unwrap(),
        "--metrics",
        metrics.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let traces_text = std::fs::read_to_string(&traces).unwrap();
    assert!(traces_text.contains("resourceSpans"), "{traces_text}");
    assert!(traces_text.contains("\"name\":\"main\""), "{traces_text}");
    assert!(traces_text.contains("service.name"), "{traces_text}");

    let metrics_text = std::fs::read_to_string(&metrics).unwrap();
    assert!(metrics_text.contains("sic.workflow.runs"), "{metrics_text}");

    for path in [src, journal, traces, metrics] {
        std::fs::remove_file(path).ok();
    }
}

#[test]
fn a_truncated_journal_still_exports_what_it_has() {
    let src = write_temp("export-cut.sic", "fn main() -> Int { return 1; }\n");
    let journal = src.with_extension("jsonl");
    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);

    // Cut the last line in half, as a killed run would leave it.
    let text = std::fs::read_to_string(&journal).unwrap();
    let cut = &text[..text.len() - 20];
    std::fs::write(&journal, cut).unwrap();

    let (stdout, stderr, code) = sic(&["export", journal.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stderr.contains("skipped line"), "{stderr}");
    assert!(stdout.contains("resourceSpans"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_file(journal).ok();
}

#[test]
fn a_resumed_run_exports_as_one_trace() {
    let src = write_temp("export-resume.sic", APPROVAL_SRC);
    let checkpoint = src.with_extension("sicc");
    let journal = src.with_extension("jsonl");

    let (_, _, code) = sic(&[
        "run",
        src.to_str().unwrap(),
        "--checkpoint",
        checkpoint.to_str().unwrap(),
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);
    let (_, _, code) = sic(&[
        "resume",
        checkpoint.to_str().unwrap(),
        src.to_str().unwrap(),
        "--value",
        "true",
        "--journal",
        journal.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);

    let (stdout, _, code) = sic(&["export", journal.to_str().unwrap()]);
    assert_eq!(code, 0);
    // One run, so one trace id, even though it took two processes.
    let ids: std::collections::HashSet<&str> = stdout
        .match_indices("\"traceId\":\"")
        .map(|(i, m)| &stdout[i + m.len()..i + m.len() + 32])
        .collect();
    assert_eq!(ids.len(), 1, "{ids:?}");

    for path in [src, checkpoint, journal] {
        std::fs::remove_file(path).ok();
    }
}

#[test]
fn version_and_help() {
    let (stdout, _, code) = sic(&["version"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("sic 0.1.0"), "{stdout}");

    let (stdout, _, code) = sic(&["help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("sic run"), "{stdout}");
    assert!(stdout.contains("sic parse"), "{stdout}");
}
