//! End-to-end tests that run the built binary.

use std::process::Command;

fn sic(args: &[&str]) -> (String, String, i32) {
    sic_in(repo_root(), args)
}

/// Runs the binary with a working directory, for programs whose capability
/// grants name relative paths.
fn sic_in(dir: std::path::PathBuf, args: &[&str]) -> (String, String, i32) {
    sic_with_store(dir, None, args)
}

/// Runs the binary with its run store pointed somewhere, so a test never
/// records into the repository.
fn sic_with_store(
    dir: std::path::PathBuf,
    store: Option<&std::path::Path>,
    args: &[&str],
) -> (String, String, i32) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sic"));
    command.args(args).current_dir(dir);
    if let Some(store) = store {
        command.env("SIC_RUNS", store);
    }
    let out = command.output().expect("failed to run sic");
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

// ---- planning ----

#[test]
fn a_plan_lists_effects_without_causing_any() {
    // The file the program would write must not exist afterwards: a plan on a
    // program nobody trusts yet is the only time a plan is worth having.
    let target = write_temp("plan-target.txt", "");
    std::fs::remove_file(&target).ok();
    let target = target.to_str().unwrap().to_string();

    let src = write_temp(
        "plan.sic",
        &format!(
            "allow {{ fs.write {target:?}; }}\n\
             fn main() {{ fs.write({target:?}, \"data\"); }}\n"
        ),
    );

    let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("WRITE"), "{stdout}");
    assert!(stdout.contains("fs.write"), "{stdout}");
    assert!(stdout.contains("bytecode sha256:"), "{stdout}");
    assert!(
        !std::path::Path::new(&target).exists(),
        "the plan wrote the file"
    );

    std::fs::remove_file(src).ok();
}

#[test]
fn a_plan_can_be_made_from_bytecode_alone() {
    // The thing you plan should be the thing you run, and bytecode from
    // somewhere else has no source to consult.
    let out = write_temp("plan.sicb", "");
    let out_str = out.to_str().unwrap().to_string();
    let (_, stderr, code) = sic(&["compile", &example("agent.sic"), "-o", &out_str]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let (stdout, _, code) = sic(&["plan", &out_str]);
    assert_eq!(code, 0);
    assert!(stdout.contains("INVOKE"), "{stdout}");
    assert!(stdout.contains("VERIFY Diagnosis"), "{stdout}");
    assert!(stdout.contains("at most 2 in a run"), "{stdout}");

    std::fs::remove_file(out).ok();
}

#[test]
fn a_plan_says_when_a_bound_is_not_one() {
    // `retry` bounds one visit, not a run, and saying otherwise would be a
    // guess dressed as a fact.
    let (stdout, _, code) = sic(&["plan", &example("tasks.sic")]);
    assert_eq!(code, 0);
    assert!(stdout.contains("none with a budget"), "{stdout}");
    assert!(stdout.contains("SPAWN"), "{stdout}");
}

#[test]
fn a_program_with_no_effects_plans_to_nothing() {
    let (stdout, _, code) = sic(&["plan", &example("milestone.sic")]);
    assert_eq!(code, 0);
    assert!(stdout.contains("(no external effects)"), "{stdout}");
    assert!(stdout.contains("No capability calls."), "{stdout}");
}

// ---- trust and provenance ----

#[test]
fn a_models_answer_reaches_a_deploy_only_through_an_approval() {
    let first = write_temp("trust-1.sicc", "");
    let second = write_temp("trust-2.sicc", "");

    // Stop 1: the model.
    let (_, stderr, code) = sic(&[
        "run",
        &example("approval-flow.sic"),
        "--checkpoint",
        first.to_str().unwrap(),
    ]);
    assert_eq!(code, 3, "stderr: {stderr}");

    // Stop 2: the person.
    let (_, stderr, code) = sic(&[
        "resume",
        first.to_str().unwrap(),
        &example("approval-flow.sic"),
        "--value",
        r#"{"action": "restart the service"}"#,
        "--checkpoint",
        second.to_str().unwrap(),
    ]);
    assert_eq!(code, 3, "stderr: {stderr}");
    assert!(stderr.contains("[deploying] deploy this?"), "{stderr}");

    let (stdout, stderr, code) = sic(&[
        "resume",
        second.to_str().unwrap(),
        &example("approval-flow.sic"),
        "--value",
        "true",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "0\n");

    std::fs::remove_file(first).ok();
    std::fs::remove_file(second).ok();
}

#[test]
fn refusing_an_approval_fails_the_run() {
    // There is no third outcome to return: without an option type, "approved
    // or not" would be a Bool beside the value that nothing forces you to read.
    let first = write_temp("refuse-1.sicc", "");
    let second = write_temp("refuse-2.sicc", "");
    let (_, _, code) = sic(&[
        "run",
        &example("approval-flow.sic"),
        "--checkpoint",
        first.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);
    let (_, _, code) = sic(&[
        "resume",
        first.to_str().unwrap(),
        &example("approval-flow.sic"),
        "--value",
        r#"{"action": "restart"}"#,
        "--checkpoint",
        second.to_str().unwrap(),
    ]);
    assert_eq!(code, 3);

    let (_, stderr, code) = sic(&[
        "resume",
        second.to_str().unwrap(),
        &example("approval-flow.sic"),
        "--value",
        "false",
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("approval was refused"), "{stderr}");

    std::fs::remove_file(first).ok();
    std::fs::remove_file(second).ok();
}

#[test]
fn passing_a_models_answer_straight_to_a_deploy_does_not_compile() {
    let source = std::fs::read_to_string(example("approval-flow.sic")).unwrap();
    let without_approval = source.replace(
        "let approved = approve(\"deploy this?\", plan);",
        "let approved = plan;",
    );
    let src = write_temp("trust-bad.sic", &without_approval);

    let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("expected HumanApproved<Plan>, found LLM<Plan>"),
        "{stderr}"
    );
    std::fs::remove_file(src).ok();
}

// ---- recorded runs ----

/// A directory for one test's run store.
fn temp_store(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("sic-store-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_recorded_run_can_be_listed_explained_and_replayed() {
    let store = temp_store("replay");
    let src = write_temp("record.sic", "fn main() -> Int { return 6 * 7; }\n");

    let (_, stderr, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", src.to_str().unwrap(), "--record"],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stderr.contains("recorded in"), "{stderr}");

    // The bytecode is kept beside the journal: replaying needs the exact
    // program, and the file on disk now is not it.
    let dirs: Vec<_> = std::fs::read_dir(&store).unwrap().flatten().collect();
    assert_eq!(dirs.len(), 1);
    assert!(dirs[0].path().join("program.sicb").exists());
    assert!(dirs[0].path().join("journal.jsonl").exists());

    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["runs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("completed"), "{stdout}");

    let id = stdout.split_whitespace().next().unwrap().to_string();
    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["explain", &id]);
    assert_eq!(code, 0);
    assert!(stdout.contains("outcome    completed"), "{stdout}");

    // Given the same program and the same answers, the VM does the same thing.
    let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("events matched"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn a_replay_that_differs_says_where() {
    // A difference is a real finding: the VM changed, the compiler changed, or
    // something was not as deterministic as it claimed.
    let store = temp_store("differs");
    let src = write_temp(
        "replay-差.sic".replace('差', "diff").as_str(),
        "fn main() -> Int { return 1; }\n",
    );
    let other = write_temp("replay-other.sic", "fn main() -> Int { return 2; }\n");

    let (_, _, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", src.to_str().unwrap(), "--record"],
    );
    assert_eq!(code, 0);

    // Swap the recorded bytecode for a different program.
    let dir = std::fs::read_dir(&store)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let other_bytecode = other.with_extension("sicb");
    let (_, _, code) = sic(&[
        "compile",
        other.to_str().unwrap(),
        "-o",
        other_bytecode.to_str().unwrap(),
    ]);
    assert_eq!(code, 0);
    std::fs::copy(&other_bytecode, dir.join("program.sicb")).unwrap();

    let id = dir.file_name().unwrap().to_string_lossy().into_owned();
    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
    assert_eq!(code, 1);
    assert!(stdout.contains("recorded"), "{stdout}");
    assert!(stdout.contains("replayed"), "{stdout}");

    for path in [src, other, other_bytecode] {
        std::fs::remove_file(path).ok();
    }
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn a_run_that_was_not_recorded_leaves_nothing_behind() {
    // Recording is opt-in, and a run that was not asked to keep anything does
    // not.
    let store = temp_store("norecord");
    let src = write_temp("norecord.sic", "fn main() -> Int { return 1; }\n");
    let (_, _, code) = sic_with_store(repo_root(), Some(&store), &["run", src.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 0);

    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["runs"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("no recorded runs"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn an_unknown_run_id_says_so() {
    let store = temp_store("unknown");
    let (_, stderr, code) = sic_with_store(repo_root(), Some(&store), &["explain", "deadbeef"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no run in"), "{stderr}");
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn inspect_run_prints_every_event() {
    let store = temp_store("inspect");
    let src = write_temp("inspect.sic", "fn main() -> Int { return 1; }\n");
    let (_, _, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", src.to_str().unwrap(), "--record"],
    );
    assert_eq!(code, 0);

    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
    let id = stdout.split_whitespace().next().unwrap().to_string();
    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["inspect-run", &id]);
    assert_eq!(code, 0);
    // Every line is an event, including the ones `explain` leaves out.
    assert!(stdout.contains("function_entered"), "{stdout}");
    assert!(stdout.contains("run_completed"), "{stdout}");

    std::fs::remove_file(src).ok();
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn a_waiting_run_is_found_and_answered_by_its_id_alone() {
    // Everything a run needs to be picked up is in its directory, so nothing
    // about a path has to be remembered - which is what makes this usable by
    // something driving `sic` rather than a person who just typed the command.
    let store = temp_store("attach");
    let (_, stderr, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", &example("approval-flow.sic"), "--record"],
    );
    assert_eq!(code, 3, "stderr: {stderr}");
    assert!(stderr.contains("sic attach"), "{stderr}");

    // What is waiting, and for what.
    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("llm.invoke"), "{stdout}");
    assert!(stdout.contains("what should we deploy?"), "{stdout}");
    let id = stdout.split_whitespace().next().unwrap().to_string();

    // Reading the question is separate from answering it: whatever answers has
    // to be able to find out what the question is first.
    let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["attach", &id]);
    assert_eq!(code, 3);
    assert!(stdout.contains("waiting: [claude-opus-4]"), "{stdout}");
    assert!(stdout.contains("--value <String>"), "{stdout}");

    // Answer the model; the run stops again, this time for a person.
    let (_, stderr, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["attach", &id, "--value", r#"{"action": "restart"}"#],
    );
    assert_eq!(code, 3, "stderr: {stderr}");
    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
    assert!(stdout.contains("human.approve"), "{stdout}");

    // Approve it, and it finishes.
    let (stdout, stderr, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["attach", &id, "--value", "true"],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "0\n");

    // A finished run is no longer waiting, and its checkpoint is gone.
    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
    assert!(stdout.contains("nothing is waiting"), "{stdout}");
    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
    assert!(stdout.contains("completed"), "{stdout}");

    std::fs::remove_dir_all(store).ok();
}

#[test]
fn attaching_to_a_run_that_is_not_waiting_says_so() {
    let store = temp_store("attach-done");
    let src = write_temp("attach-done.sic", "fn main() -> Int { return 1; }\n");
    let (_, _, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", src.to_str().unwrap(), "--record"],
    );
    assert_eq!(code, 0);

    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
    let id = stdout.split_whitespace().next().unwrap().to_string();
    let (_, stderr, code) = sic_with_store(repo_root(), Some(&store), &["attach", &id]);
    assert_eq!(code, 2);
    assert!(stderr.contains("not waiting"), "{stderr}");

    std::fs::remove_file(src).ok();
    std::fs::remove_dir_all(store).ok();
}

#[test]
fn an_answered_run_still_replays() {
    // An answer given through `attach` is recorded like any other, so the run
    // stays replayable.
    let store = temp_store("attach-replay");
    let (_, _, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &["run", &example("agent.sic"), "--record"],
    );
    assert_eq!(code, 3);

    let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
    let id = stdout.split_whitespace().next().unwrap().to_string();
    let (_, stderr, code) = sic_with_store(
        repo_root(),
        Some(&store),
        &[
            "attach",
            &id,
            "--value",
            r#"{"cause": "disk full", "confidence": 0.9}"#,
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");

    let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("events matched"), "{stdout}");

    std::fs::remove_dir_all(store).ok();
}

#[test]
fn every_example_compiles_verifies_and_plans() {
    // `plan` runs the whole front end and the verifier and executes nothing, so
    // it is the cheapest way to say that every example in the repository still
    // means something.
    let dir = repo_root().join("examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir)
        .expect("examples/ should exist")
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "sic") {
            continue;
        }
        let name = path.to_string_lossy().into_owned();
        let (stdout, stderr, code) = sic(&["plan", &name]);
        assert_eq!(code, 0, "{name} does not plan: {stderr}");
        assert!(stdout.contains("Execution plan for"), "{name}: {stdout}");
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} examples were checked");
}

#[test]
fn version_and_help() {
    let (stdout, _, code) = sic(&["version"]);
    assert_eq!(code, 0);
    assert!(
        stdout.starts_with(concat!("sic ", env!("CARGO_PKG_VERSION"))),
        "{stdout}"
    );

    let (stdout, _, code) = sic(&["help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("sic run"), "{stdout}");
    assert!(stdout.contains("sic parse"), "{stdout}");
}

// ---- modules ----

/// Writes a small program made of several files, and returns the entry file.
fn write_temp_program(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    for (rel, contents) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
    }
    dir.join(files[0].0)
}

const LIB_DEPLOY: &str = "\
requires {
    process.exec;
}

fn deploy(binary: String) -> Int {
    return process.exec(binary);
}
";

#[test]
fn an_imported_function_is_callable() {
    let entry = write_temp_program(
        "import-ok",
        &[
            (
                "main.sic",
                "import \"./lib/deploy.sic\";\n\n\
                 allow {\n    process.exec \"/bin/echo\";\n}\n\n\
                 fn main() -> Int {\n    return deploy(\"/bin/echo\");\n}\n",
            ),
            ("lib/deploy.sic", LIB_DEPLOY),
        ],
    );
    let (stdout, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout.trim(), "0");
}

#[test]
fn a_plan_says_which_file_asks_for_a_grant() {
    let entry = write_temp_program(
        "import-plan",
        &[
            (
                "main.sic",
                "import \"./lib/deploy.sic\";\n\n\
                 allow {\n    process.exec \"/bin/echo\";\n}\n\n\
                 fn main() -> Int {\n    return deploy(\"/bin/echo\");\n}\n",
            ),
            ("lib/deploy.sic", LIB_DEPLOY),
        ],
    );
    let (stdout, stderr, code) = sic(&["plan", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("called from"), "{stdout}");
    assert!(stdout.contains("lib/deploy.sic"), "{stdout}");
}

#[test]
fn a_required_capability_has_to_be_granted() {
    let entry = write_temp_program(
        "import-ungranted",
        &[
            (
                "main.sic",
                "import \"./lib/deploy.sic\";\n\n\
                 fn main() -> Int {\n    return deploy(\"/bin/echo\");\n}\n",
            ),
            ("lib/deploy.sic", LIB_DEPLOY),
        ],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0404"), "{stderr}");
}

#[test]
fn a_requirement_nothing_calls_is_a_warning() {
    let entry = write_temp_program(
        "import-unused-requires",
        &[
            (
                "main.sic",
                "import \"./lib/idle.sic\";\n\n\
                 allow {\n    process.exec \"/bin/echo\";\n}\n\n\
                 fn main() -> Int {\n    return quiet();\n}\n",
            ),
            (
                "lib/idle.sic",
                "requires {\n    process.exec;\n}\n\nfn quiet() -> Int {\n    return 0;\n}\n",
            ),
        ],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stderr.contains("E0405"), "{stderr}");
}

#[test]
fn a_file_either_grants_or_requires() {
    let entry = write_temp_program(
        "import-both-roles",
        &[
            (
                "main.sic",
                "import \"./lib/greedy.sic\";\n\nfn main() -> Int {\n    return go();\n}\n",
            ),
            (
                "lib/greedy.sic",
                "allow {\n    process.exec \"/bin/echo\";\n}\n\n\
                 requires {\n    process.exec;\n}\n\n\
                 fn go() -> Int {\n    return process.exec(\"/bin/echo\");\n}\n",
            ),
        ],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0403"), "{stderr}");
}

#[test]
fn an_import_cycle_is_reported_rather_than_followed() {
    let entry = write_temp_program(
        "import-cycle",
        &[
            (
                "main.sic",
                "import \"./a.sic\";\n\nfn main() -> Int {\n    return one();\n}\n",
            ),
            (
                "a.sic",
                "import \"./b.sic\";\n\nfn one() -> Int {\n    return two();\n}\n",
            ),
            (
                "b.sic",
                "import \"./a.sic\";\n\nfn two() -> Int {\n    return 2;\n}\n",
            ),
        ],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0402"), "{stderr}");
}

#[test]
fn an_import_may_not_reach_outside_the_program() {
    let entry = write_temp_program(
        "import-escape",
        &[(
            "main.sic",
            "import \"../elsewhere.sic\";\n\nfn main() -> Int {\n    return 0;\n}\n",
        )],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0400"), "{stderr}");
}

#[test]
fn an_import_that_is_not_there_names_the_import() {
    let entry = write_temp_program(
        "import-missing",
        &[(
            "main.sic",
            "import \"./nowhere.sic\";\n\nfn main() -> Int {\n    return 0;\n}\n",
        )],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("E0401"), "{stderr}");
    assert!(stderr.contains("nowhere.sic"), "{stderr}");
}

#[test]
fn the_same_file_imported_twice_comes_in_once() {
    let entry = write_temp_program(
        "import-diamond",
        &[
            (
                "main.sic",
                "import \"./a.sic\";\nimport \"./b.sic\";\n\n\
                 fn main() -> Int {\n    return one() + two();\n}\n",
            ),
            (
                "a.sic",
                "import \"./shared.sic\";\n\nfn one() -> Int {\n    return base();\n}\n",
            ),
            (
                "b.sic",
                "import \"./shared.sic\";\n\nfn two() -> Int {\n    return base();\n}\n",
            ),
            ("shared.sic", "fn base() -> Int {\n    return 21;\n}\n"),
        ],
    );
    let (stdout, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn a_failure_inside_an_imported_file_names_that_file() {
    let entry = write_temp_program(
        "import-failure",
        &[
            (
                "main.sic",
                "import \"./lib/math.sic\";\n\nfn main() -> Int {\n    return half(0);\n}\n",
            ),
            (
                "lib/math.sic",
                "fn half(n: Int) -> Int {\n    return 10 / n;\n}\n",
            ),
        ],
    );
    let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains("lib/math.sic:2"), "{stderr}");
}

// ---- update ----

/// Copies the built binary into a directory of its own, so that a test which
/// replaces a running binary replaces that copy and not the one cargo built.
fn install_copy(name: &str, rel: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_sic"), &path).unwrap();
    path
}

fn digest_of(path: &std::path::Path) -> String {
    sic_core::Digest::of(&std::fs::read(path).unwrap()).hex()
}

/// Runs a binary by its own path, which is what `sic update` needs: the file it
/// replaces is the one it is running from.
fn sic_at(binary: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let out = Command::new(binary)
        .args(args)
        .output()
        .expect("failed to run sic");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn update_check_says_what_is_installed() {
    let (stdout, stderr, code) = sic(&["update", "--check"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.starts_with(concat!(
            "  installed  ",
            env!("CARGO_PKG_VERSION"),
            "  sha256:"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("sic"), "{stdout}");
}

#[test]
fn an_update_without_a_digest_is_refused() {
    let (_, stderr, code) = sic(&["update", "--to", env!("CARGO_BIN_EXE_sic")]);
    assert_eq!(code, 2);
    assert!(stderr.contains("--sha256"), "{stderr}");
}

#[test]
fn a_digest_that_does_not_match_refuses_the_update() {
    let zeros = "0".repeat(64);
    let (_, stderr, code) = sic(&[
        "update",
        "--to",
        env!("CARGO_BIN_EXE_sic"),
        "--sha256",
        &zeros,
    ]);
    assert_eq!(code, 1);
    assert!(stderr.contains("but --sha256 says"), "{stderr}");
}

/// Linux only: the candidate is the binary with a byte appended, which an ELF
/// loader ignores but a signed Mach-O does not.
#[cfg(target_os = "linux")]
#[test]
fn an_update_replaces_the_running_binary() {
    let installed = install_copy("update-swap", "sic");
    let candidate = installed.with_file_name("candidate");
    let mut bytes = std::fs::read(&installed).unwrap();
    bytes.push(b'\n');
    std::fs::write(&candidate, &bytes).unwrap();
    let digest = digest_of(&candidate);

    let (stdout, stderr, code) = sic_at(
        &installed,
        &[
            "update",
            "--to",
            candidate.to_str().unwrap(),
            "--sha256",
            &digest,
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("replaced"), "{stdout}");
    assert_eq!(digest_of(&installed), digest);
    assert_eq!(leftovers(installed.parent().unwrap()), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn a_check_replaces_nothing() {
    let installed = install_copy("update-check", "sic");
    let candidate = installed.with_file_name("candidate");
    let mut bytes = std::fs::read(&installed).unwrap();
    bytes.push(b'\n');
    std::fs::write(&candidate, &bytes).unwrap();
    let before = digest_of(&installed);
    let digest = digest_of(&candidate);

    let (stdout, stderr, code) = sic_at(
        &installed,
        &[
            "update",
            "--check",
            "--to",
            candidate.to_str().unwrap(),
            "--sha256",
            &digest,
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("would replace"), "{stdout}");
    assert_eq!(digest_of(&installed), before);
    // The staged file is gone: a check leaves the directory as it found it.
    assert_eq!(leftovers(installed.parent().unwrap()), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn a_candidate_that_is_not_sic_is_refused() {
    let installed = install_copy("update-not-sic", "sic");
    let candidate = installed.with_file_name("impostor");
    std::fs::write(&candidate, "#!/bin/sh\necho not sic at all\n").unwrap();
    let digest = digest_of(&candidate);
    let before = digest_of(&installed);

    let (_, stderr, code) = sic_at(
        &installed,
        &[
            "update",
            "--to",
            candidate.to_str().unwrap(),
            "--sha256",
            &digest,
        ],
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains("does not identify itself as sic"),
        "{stderr}"
    );
    assert_eq!(digest_of(&installed), before);
    assert_eq!(leftovers(installed.parent().unwrap()), 2);
}

#[cfg(target_os = "linux")]
#[test]
fn a_binary_a_package_manager_installed_is_left_alone() {
    let installed = install_copy("update-cargo", ".cargo/bin/sic");
    let candidate = installed.with_file_name("candidate");
    let mut bytes = std::fs::read(&installed).unwrap();
    bytes.push(b'\n');
    std::fs::write(&candidate, &bytes).unwrap();
    let digest = digest_of(&candidate);
    let before = digest_of(&installed);

    let (_, stderr, code) = sic_at(
        &installed,
        &[
            "update",
            "--to",
            candidate.to_str().unwrap(),
            "--sha256",
            &digest,
        ],
    );
    assert_eq!(code, 1);
    assert!(stderr.contains("installed by cargo"), "{stderr}");
    assert_eq!(digest_of(&installed), before);
}

fn leftovers(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}
