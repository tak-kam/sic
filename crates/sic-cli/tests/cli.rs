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
///
/// The path is this process and `name`, so **two tests that pass the same
/// name share a file** - and tests run at the same time as each other, so one
/// deletes the other's while it is being read. Give each test its own name.
fn write_temp(name: &str, contents: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

fn example(name: &str) -> String {
    format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))
}

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

/// A program that asks a person, and does something different with each answer.
const APPROVAL_SRC: &str = "allow { human.approve \"a test\"; }\n\
fn main() -> Int {\n\
    let ok = human.approve(\"go ahead?\");\n\
    if ok { return 1; }\n\
    return 0;\n\
}\n";

/// A directory for one test's run store.
fn temp_store(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("sic-store-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

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

/// A directory of its own for one test, emptied first so a rerun starts
/// from nothing.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("sic-test-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a writable temporary directory");
    dir
}

/// the interpreter in a process of its own.
///
/// Asking the person who is sitting in front of the run, instead of leaving it
/// for whoever comes along later. `docs/design/interactive.md`.
///
/// What is *not* here is the loop itself. Reading an answer needs a terminal,
/// and a terminal cannot be made without `ioctl`, which cannot be reached
/// without a dependency - the same reason the `--llm tmux:` tests check the
/// refusals rather than driving an agent. So the reading is unit-tested in
/// `cmd::ask` against a slice, and what is checked here is that a machine with
/// nobody in front of it finds out at once.
mod interactive {
    use super::*;

    /// The whole of §6 in one behaviour: a CI job that inherited the flag has
    /// to fail in a second rather than hang until somebody notices a queue.
    #[test]
    fn a_run_with_nobody_in_front_of_it_says_so_rather_than_waiting() {
        let store = temp_store("interactive-no-tty");
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("approval.sic"), "--record", "--interactive"],
        );
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("needs a terminal"), "{stderr}");
        std::fs::remove_dir_all(store).ok();
    }

    /// And before the program runs, not at the first question: a run that
    /// performed three effects and then found nobody to ask is a run that has
    /// to be picked up by hand, which is what the flag was for.
    #[test]
    fn nothing_runs_before_the_terminal_is_looked_for() {
        let store = temp_store("interactive-nothing-ran");
        let (_, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("approval.sic"), "--record", "--interactive"],
        );
        assert_eq!(code, 2);
        // A recorded run creates its directory before the first instruction,
        // so an empty store is proof that nothing started.
        let started = std::fs::read_dir(&store).map(|d| d.count()).unwrap_or(0);
        assert_eq!(started, 0, "the run was created anyway");
        std::fs::remove_dir_all(store).ok();
    }

    /// A question on the screen is only free because the run is already on
    /// disk, and the store is where the answer's reason goes. Without
    /// `--record` there is neither, so the flag is refused rather than made to
    /// half-work.
    ///
    /// Checked before the terminal is, because what was typed is wrong or
    /// right on its own and being told to add `--record` only after the
    /// terminal question is settled would be two round trips for one command
    /// line - which is also what makes this testable without a terminal.
    #[test]
    fn a_run_that_is_not_kept_has_nowhere_to_be_asked_about() {
        let (_, stderr, code) = sic(&["run", &example("approval.sic"), "--interactive"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("needs --record"), "{stderr}");
        assert!(!stderr.contains("needs a terminal"), "{stderr}");
    }

    /// `attach` takes it on the same terms, and finds out at the same point.
    #[test]
    fn picking_a_run_up_interactively_needs_a_terminal_too() {
        let (_, stderr, code) = sic(&["attach", "00000000", "--interactive"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("needs a terminal"), "{stderr}");
    }

    /// `resume` deliberately does not take it: a loose checkpoint has nowhere
    /// to record a reason, does not say which run's conversation it belongs
    /// to, and would need its next destination named again.
    /// `docs/design/interactive.md` §4.
    #[test]
    fn a_loose_checkpoint_is_not_answered_from_a_terminal() {
        let (_, stderr, code) = sic(&[
            "resume",
            "nothing.sicc",
            &example("approval.sic"),
            "--value",
            "true",
            "--interactive",
        ]);
        assert_eq!(code, 2, "{stderr}");
        assert!(
            stderr.contains("nowhere to keep the answer's reason"),
            "{stderr}"
        );
        // A refusal with a reason, not an option nobody has heard of.
        assert!(!stderr.contains("unknown option"), "{stderr}");
    }
}

/// `docs/design/processes.md`. What the split buys today is a resource bound;
/// what it makes possible later is a child with fewer privileges than the
/// parent, which is the one thing a crate boundary cannot do.
#[cfg(unix)]
mod isolated {
    use super::*;

    /// Runs the binary with somewhere else to put its socket, which is the
    /// only part of a run that has a temporary directory in it.
    fn sic_with_tmpdir(tmp: &std::path::Path, args: &[&str]) -> (String, String, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_sic"))
            .args(args)
            .current_dir(repo_root())
            .env("TMPDIR", tmp)
            .output()
            .expect("failed to run sic");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// The default is a process of its own, and nothing about a run that
    /// succeeds can say so: the whole design is that a checkpoint, a journal
    /// and an answer are the same either way. What differs is what a run
    /// *needs*, and only the split needs a socket. So point the socket
    /// somewhere that does not exist and see which shape notices.
    #[test]
    fn the_interpreter_gets_its_own_process_without_being_asked() {
        let nowhere = repo_root().join("no-such-directory-for-a-socket");
        let (_, stderr, code) = sic_with_tmpdir(&nowhere, &["run", &example("milestone.sic")]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("cannot listen at"), "{stderr}");

        // And the refusal names the way out, which is the only reason it is
        // allowed to refuse rather than fall back. `docs/design/processes.md`
        // §7.
        assert!(stderr.contains("--no-isolate"), "{stderr}");
        let (stdout, stderr, code) = sic_with_tmpdir(
            &nowhere,
            &["run", &example("milestone.sic"), "--no-isolate"],
        );
        assert_eq!(code, 0, "{stderr}");
        assert!(!stdout.is_empty(), "it ran");
    }

    /// `--isolate` asked for what now happens anyway, so it still works; and
    /// when both are on one command line the refusal wins, because it is the
    /// one that can always be honoured.
    #[test]
    fn the_old_flag_still_asks_for_what_is_now_the_default() {
        let nowhere = repo_root().join("no-such-directory-for-a-socket");
        let (_, stderr, code) =
            sic_with_tmpdir(&nowhere, &["run", &example("milestone.sic"), "--isolate"]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("cannot listen at"), "{stderr}");

        for both in [["--isolate", "--no-isolate"], ["--no-isolate", "--isolate"]] {
            let (stdout, stderr, code) = sic_with_tmpdir(
                &nowhere,
                &["run", &example("milestone.sic"), both[0], both[1]],
            );
            assert_eq!(code, 0, "{both:?}: {stderr}");
            assert!(!stdout.is_empty(), "{both:?}: it ran");
        }
    }

    /// The same answer either way, which is the first thing to be sure of: a
    /// second shape that computes something else is not a second shape.
    #[test]
    fn a_run_gives_the_same_answer_in_one_process_or_two() {
        for name in ["milestone.sic", "records.sic", "branching.sic"] {
            let (one, stderr, code) = sic(&["run", &example(name), "--no-isolate"]);
            assert_eq!(code, 0, "{name}: {stderr}");
            let (two, stderr, code) = sic(&["run", &example(name), "--isolate"]);
            assert_eq!(code, 0, "{name}: {stderr}");
            assert_eq!(one, two, "{name}");
        }
    }

    /// The child asks and the parent performs. Nothing in the child opens a
    /// file; the answer came back over the socket.
    #[test]
    fn a_capability_call_crosses_the_wire_and_is_performed_by_the_parent() {
        let (stdout, stderr, code) = sic(&["run", &example("read-file.sic"), "--isolate"]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "\"hello from a file\\n\"");
    }

    /// The events crossed whole. A journal that differed between the shapes
    /// would mean the wire lost something, and the one thing it must not lose
    /// is what `sic explain` and the exporter read.
    #[test]
    fn the_journal_is_the_same_journal() {
        let store = temp_store("isolate-journal");
        std::fs::create_dir_all(&store).ok();
        let one = store.join("one.jsonl");
        let two = store.join("two.jsonl");
        for (path, args) in [
            (&one, vec!["run", &example("read-file.sic"), "--journal"]),
            (
                &two,
                vec!["run", &example("read-file.sic"), "--isolate", "--journal"],
            ),
        ] {
            let mut args = args;
            let shown = path.to_string_lossy().into_owned();
            args.push(&shown);
            let (_, stderr, code) = sic(&args);
            assert_eq!(code, 0, "{stderr}");
        }
        // The run id and the wall clock differ between two runs of anything;
        // everything else is the run.
        let strip = |path: &std::path::Path| -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| {
                    let json = sic_json::parse(line).expect("a journal line");
                    let sic_json::Json::Object(members) = json else {
                        panic!("a line that is not an object: {line}");
                    };
                    members
                        .iter()
                        .filter(|(name, _)| name != "ts" && name != "run")
                        .map(|(name, value)| format!("{name}={value:?}"))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .collect()
        };
        assert_eq!(strip(&one), strip(&two));
        assert!(!strip(&one).is_empty());
        std::fs::remove_dir_all(store).ok();
    }

    /// The failure is rendered by the child, because the value and the source
    /// position live in its arena. It has to read the same.
    #[test]
    fn a_failure_reads_the_same_from_either_side_of_the_wire() {
        let src = write_temp("isolate-fail.sic", "fn main() -> Int { return 1 / 0; }\n");
        let (_, one, code) = sic(&["run", src.to_str().unwrap(), "--no-isolate"]);
        assert_eq!(code, 1);
        let (_, two, code) = sic(&["run", src.to_str().unwrap(), "--isolate"]);
        assert_eq!(code, 1);
        assert_eq!(one, two, "the child renders it and the parent prints it");
        assert!(one.contains("division by zero"), "{one}");
        std::fs::remove_file(src).ok();
    }

    /// The state is in the child and the filesystem is in the parent, so the
    /// child produces the bytes and the parent writes them. A run that stops to
    /// wait has to be as resumable as one that never left this process.
    #[test]
    fn a_run_that_stops_to_wait_is_saved_and_picked_up_again() {
        let store = temp_store("isolate-wait");
        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("approval.sic"), "--isolate", "--record"],
        );
        assert_eq!(code, 3, "{stderr}\n{stdout}");
        assert!(stderr.contains("waiting: "), "{stderr}");
        assert!(stderr.contains("saved "), "{stderr}");

        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        let id = stdout.split_whitespace().next().unwrap().to_string();
        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["attach", &id, "--value", "true"],
        );
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "0");
        std::fs::remove_dir_all(store).ok();
    }

    /// A run with nowhere to put its state is the same refusal in either
    /// shape, and the words are the same words.
    #[test]
    fn a_waiting_run_with_nowhere_to_be_saved_says_so() {
        let (_, stderr, code) = sic(&["run", &example("approval.sic"), "--isolate"]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("has nowhere to be saved"), "{stderr}");
        let (_, one, _) = sic(&["run", &example("approval.sic"), "--no-isolate"]);
        assert_eq!(one, stderr, "the two shapes say the same thing");
    }

    /// And the bytes are the bytes. A checkpoint written by the child has to
    /// be the one the parent would have written, or `sic resume` is checking a
    /// digest against a program it does not describe.
    #[test]
    fn the_checkpoint_is_the_same_checkpoint() {
        let store = temp_store("isolate-checkpoint");
        std::fs::create_dir_all(&store).ok();
        let one = store.join("one.sicc");
        let two = store.join("two.sicc");
        let program = example("approval.sic");
        for (path, how) in [(&one, "--no-isolate"), (&two, "--isolate")] {
            let shown = path.to_string_lossy().into_owned();
            let args = vec!["run", program.as_str(), "--checkpoint", shown.as_str(), how];
            let (_, stderr, code) = sic(&args);
            assert_eq!(code, 3, "{stderr}");
        }
        let a = std::fs::read(&one).unwrap();
        let b = std::fs::read(&two).unwrap();
        // Everything but the run id, which differs between any two runs of
        // anything. The digest of the bytecode is bytes 8 to 40, and it is what
        // `resume` checks.
        assert_eq!(a[..40], b[..40], "the header and the program digest");
        assert_eq!(a[56..], b[56..], "the state");
        assert_eq!(a.len(), b.len());
        std::fs::remove_dir_all(store).ok();
    }

    /// `sic vm` is started by a run, not by a person, and says so rather than
    /// waiting for a socket nobody is listening on.
    #[test]
    fn the_interpreter_says_so_when_there_is_no_run_to_reach() {
        let (_, stderr, code) = sic(&["vm", "--socket", "/nonexistent/sic.sock"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("cannot reach the run"), "{stderr}");
    }

    /// A parent that dies leaves a child waiting on a socket nobody will write
    /// to. It notices, because that is the only thing it ever waits on: the
    /// far end closing is a read that ends.
    ///
    /// This matters more than it looks. An interpreter left running with
    /// nobody reading its socket is the failure the whole arrangement is meant
    /// to bound, so it is checked rather than reasoned about.
    #[test]
    fn the_interpreter_leaves_when_the_run_does() {
        use std::os::unix::net::UnixListener;

        let path = std::env::temp_dir().join(format!("sic-orphan-{}.sock", std::process::id()));
        std::fs::remove_file(&path).ok();
        let listener = UnixListener::bind(&path).expect("a socket");

        let child = Command::new(env!("CARGO_BIN_EXE_sic"))
            .args(["vm", "--socket"])
            .arg(&path)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the interpreter starts");

        // It connects and waits for a program. Dropping the connection is what
        // a parent that died looks like from over there.
        let (stream, _) = listener.accept().expect("it connects");
        drop(stream);
        drop(listener);

        let out = child.wait_with_output().expect("it ends");
        assert!(!out.status.success(), "it should not report success");
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains("the run went away"), "{said}");

        std::fs::remove_file(&path).ok();
    }

    /// A checkpoint does not remember which shape wrote it, so the four
    /// combinations have to work and give the same answer. That is what makes
    /// `--isolate` a way of running rather than a kind of run.
    #[test]
    fn a_checkpoint_does_not_care_which_shape_wrote_it() {
        for (n, (writing, reading)) in [
            ("--no-isolate", "--no-isolate"),
            ("--isolate", "--no-isolate"),
            ("--no-isolate", "--isolate"),
            ("--isolate", "--isolate"),
        ]
        .into_iter()
        .enumerate()
        {
            let store = temp_store(&format!("cross{n}"));
            let program = example("approval.sic");
            let run = vec!["run", program.as_str(), "--record", writing];
            let (_, stderr, code) = sic_with_store(repo_root(), Some(&store), &run);
            assert_eq!(code, 3, "writing {writing:?}: {stderr}");

            let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
            let id = stdout.split_whitespace().next().unwrap().to_string();
            let attach = vec!["attach", id.as_str(), "--value", "true", reading];
            let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &attach);
            assert_eq!(code, 0, "{writing:?} then {reading:?}: {stderr}");
            assert_eq!(stdout.trim(), "0", "{writing:?} then {reading:?}");
            std::fs::remove_dir_all(store).ok();
        }
    }

    /// `resume` too, which has no run directory: the answer is shaped from the
    /// checkpoint alone, because nothing is restored on this side.
    #[test]
    fn a_loose_checkpoint_resumes_in_a_process_of_its_own() {
        let store = temp_store("isolate-resume");
        std::fs::create_dir_all(&store).ok();
        let saved = store.join("wait.sicc");
        let shown = saved.to_string_lossy().into_owned();
        let (_, stderr, code) = sic(&[
            "run",
            &example("approval.sic"),
            "--checkpoint",
            shown.as_str(),
        ]);
        assert_eq!(code, 3, "{stderr}");

        let (stdout, stderr, code) = sic(&[
            "resume",
            shown.as_str(),
            &example("approval.sic"),
            "--value",
            "true",
            "--isolate",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "0");
        std::fs::remove_dir_all(store).ok();
    }

    /// The answer is shaped on this side, from the checkpoint, so a value of
    /// the wrong shape is refused before a child is started.
    #[test]
    fn an_answer_the_call_cannot_take_is_refused_without_starting_anything() {
        let store = temp_store("isolate-wrong");
        std::fs::create_dir_all(&store).ok();
        let saved = store.join("wait.sicc");
        let shown = saved.to_string_lossy().into_owned();
        let (_, _, code) = sic(&[
            "run",
            &example("approval.sic"),
            "--checkpoint",
            shown.as_str(),
        ]);
        assert_eq!(code, 3);

        let (_, stderr, code) = sic(&[
            "resume",
            shown.as_str(),
            &example("approval.sic"),
            "--value",
            "not a bool",
            "--isolate",
        ]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("human.approve"), "{stderr}");
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn the_interpreter_needs_a_socket() {
        let (_, stderr, code) = sic(&["vm"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("`vm` takes `--socket PATH`"), "{stderr}");
    }
}

/// what a program has to say about itself.
///
/// A log line goes where a person can see it and is kept where the run is
/// kept, and the journal holds the digest of it rather than the text - which
/// is the split that let §26 be built at all. See `docs/design/logging.md`.
mod logging {
    use super::*;

    const SAYS: &str = "fn main() -> Int {\n\
                        \x20   log info \"looking\";\n\
                        \x20   log warn \"and again\";\n\
                        \x20   return 1;\n\
                        }\n";

    /// stderr, as it happens, whether or not anybody asked for a journal.
    /// stdout is the value the program returned and must not be mistakable
    /// for a line saying what happened.
    #[test]
    fn a_line_is_shown_without_anybody_asking_for_a_journal() {
        let src = write_temp("log-shown.sic", SAYS);
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stderr.contains("info: looking"), "{stderr}");
        assert!(stderr.contains("warn: and again"), "{stderr}");
        assert_eq!(stdout.trim(), "1", "the value is the only thing on stdout");
        std::fs::remove_file(src).ok();
    }

    /// The journal is the run's account and holds digests. Putting the text
    /// there would cost the rule that makes telemetry safe by default.
    #[test]
    fn the_journal_holds_the_digest_and_the_values_file_holds_the_text() {
        let store = temp_store("log-kept");
        let src = write_temp("log-kept.sic", SAYS);
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 0, "{stderr}");

        let dir = std::fs::read_dir(&store).unwrap().flatten().next().unwrap();
        let journal = std::fs::read_to_string(dir.path().join("journal.jsonl")).unwrap();
        assert!(journal.contains("\"event\":\"logged\""), "{journal}");
        assert!(journal.contains("\"level\":\"warn\""), "{journal}");
        assert!(
            !journal.contains("looking"),
            "the text is not here: {journal}"
        );
        assert!(
            journal.contains(&sic_core::Digest::of(b"looking").to_string()),
            "{journal}"
        );

        let logs = std::fs::read_to_string(dir.path().join("logs.jsonl")).unwrap();
        assert!(logs.contains("\"message\":\"looking\""), "{logs}");
        assert!(logs.contains("\"message\":\"and again\""), "{logs}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// Read back, the lines sit where they happened rather than in a section
    /// of their own: what a program said is only useful next to what it did.
    #[test]
    fn explain_shows_the_lines_where_they_happened() {
        let store = temp_store("log-explain");
        let src = write_temp(
            "log-explain.sic",
            "allow {\n\
             \x20   process.run \"/bin/echo\" args [];\n\
             }\n\
             fn main() -> Int {\n\
             \x20   log info \"before\";\n\
             \x20   let r = process.run(\"/bin/echo\", []);\n\
             \x20   log info \"after\";\n\
             \x20   return r.code;\n\
             }\n",
        );
        let (_, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 0);
        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        let id = stdout.split_whitespace().next().unwrap().to_string();

        let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &["explain", &id]);
        assert_eq!(code, 0, "{stderr}");
        let before = stdout.find("info: before").expect(&stdout);
        let call = stdout.find("call process.run").expect(&stdout);
        let after = stdout.find("info: after").expect(&stdout);
        assert!(before < call && call < after, "{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// A run nobody asked to keep keeps nothing, which is what
    /// `responses.jsonl` already promises. Saying so beats printing a digest
    /// where a sentence goes.
    #[test]
    fn a_run_that_was_not_recorded_says_the_text_was_not_kept() {
        let store = temp_store("log-unkept");
        let src = write_temp("log-unkept.sic", SAYS);
        let journal = store.join("run.jsonl");
        std::fs::create_dir_all(&store).ok();
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &[
                "run",
                src.to_str().unwrap(),
                "--journal",
                journal.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "{stderr}");
        let text = std::fs::read_to_string(&journal).unwrap();
        assert!(text.contains("\"event\":\"logged\""), "{text}");
        assert!(!text.contains("looking"), "{text}");
        assert!(!store.join("logs.jsonl").exists(), "nothing was kept");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// Any provenance, erased on the way in: logging reaches nothing outside
    /// the run's own account of itself.
    #[test]
    fn a_value_with_a_provenance_may_be_logged() {
        let src = write_temp(
            "log-trust.sic",
            "allow {\n\
             \x20   process.run \"/bin/echo\" args [];\n\
             }\n\
             fn main() -> Int {\n\
             \x20   let r = process.run(\"/bin/echo\", []);\n\
             \x20   log info r.output;\n\
             \x20   return r.code;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stderr.contains("info: "), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A message is text, whatever produced it.
    #[test]
    fn a_message_that_is_not_text_does_not_compile() {
        let src = write_temp(
            "log-int.sic",
            "fn main() -> Int {\n    log info 1;\n    return 1;\n}\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0301"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// `log info "x"` is two identifiers in a row, which no expression can be,
    /// so the parser knows it is a log statement whatever the second word is -
    /// and a mistyped level is a mistyped level rather than a parser guessing
    /// at an expression.
    #[test]
    fn a_level_that_is_not_one_of_the_four_says_so() {
        let src = write_temp(
            "log-level.sic",
            "fn main() -> Int {\n    log shout \"x\";\n    return 1;\n}\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0218"), "{stderr}");
        assert!(
            stderr.contains("`debug`, `info`, `warn` and `error`"),
            "{stderr}"
        );
        std::fs::remove_file(src).ok();
    }

    /// Nothing is reserved for it, the way nothing is reserved for `args` or
    /// `repeatable`, so a program may still have a function called `log`.
    #[test]
    fn log_is_still_a_name_a_program_may_use() {
        let src = write_temp(
            "log-name.sic",
            "fn log(x: Int) -> Int { return x; }\n\
             fn main() -> Int { return log(3); }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "3");
        std::fs::remove_file(src).ok();
    }
}

/// what the documents show against what the binary prints.
///
/// `docs/diagnostics.md` and `docs/status.md` are already checked against the
/// source. A sample output is the third kind of claim a document makes that can
/// drift, and it had no check: four documents went stale, two of them within a
/// week of the change that did it.
mod documentation {
    use super::*;

    /// The commands whose samples can be checked, which is the commands that
    /// change nothing.
    ///
    /// Not a convenience: a test that ran `sic run app.sic --record` would be
    /// a test with side effects, and the `sic run` samples in these documents
    /// name programs nobody wrote. Those are illustrations and stay
    /// illustrations - inventing example programs so that a test becomes
    /// possible is the test wagging the documentation.
    const READS_ONLY: &[&str] = &["plan", "verify", "disasm", "parse", "hir"];

    /// Every `.md` under the repository root, entered depth first.
    fn markdown(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "target" || n == ".git")
                {
                    continue;
                }
                markdown(&path, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }

    /// Whether one line of a sample matches one line of output.
    ///
    /// A sample line ending in `...` matches any output line starting with what
    /// comes before it. That is the whole of the matching language, and it is
    /// there for one thing: `bytecode sha256:...` changes whenever the compiler
    /// emits a byte differently, and a reader learns nothing from which bytes.
    fn line_matches(sample: &str, printed: &str) -> bool {
        match sample.strip_suffix("...") {
            Some(prefix) => printed.starts_with(prefix),
            None => sample == printed,
        }
    }

    #[test]
    fn every_sample_the_docs_show_is_what_the_binary_prints() {
        let root = repo_root();
        let mut files = Vec::new();
        markdown(&root, &mut files);
        files.sort();

        let mut checked = 0;
        for file in &files {
            let text = std::fs::read_to_string(file).expect("a readable document");
            let shown = file.strip_prefix(&root).unwrap_or(file).display();
            let mut lines = text.lines();
            while let Some(line) = lines.next() {
                if !line.starts_with("```") {
                    continue;
                }
                let mut block: Vec<&str> = Vec::new();
                for inner in lines.by_ref() {
                    if inner.starts_with("```") {
                        break;
                    }
                    block.push(inner);
                }
                let Some(command) = block.first().and_then(|l| l.strip_prefix("$ sic ")) else {
                    continue;
                };
                let args: Vec<&str> = command.split_whitespace().collect();
                if args.first().is_none_or(|verb| !READS_ONLY.contains(verb)) {
                    continue;
                }
                // A sample naming a file that is not here is an illustration.
                if !args[1..].iter().any(|a| root.join(a).exists()) {
                    continue;
                }
                let (stdout, stderr, code) = sic(&args);
                assert_eq!(code, 0, "{shown}: `sic {command}` failed: {stderr}");

                let expected: Vec<&str> = block[1..]
                    .iter()
                    .copied()
                    .rev()
                    .skip_while(|l| l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let printed: Vec<&str> = stdout.lines().collect();
                assert_eq!(
                    expected.len(),
                    printed.len(),
                    "{shown} shows {} lines for `sic {command}` and it printed {}:\n{stdout}",
                    expected.len(),
                    printed.len()
                );
                for (i, (sample, out)) in expected.iter().zip(&printed).enumerate() {
                    assert!(
                        line_matches(sample, out),
                        "{shown}, line {} of `sic {command}`:\n  shows    {sample:?}\n  \
                         it prints {out:?}",
                        i + 1
                    );
                }
                checked += 1;
            }
        }
        // A check that covers nothing reports the same green as one that
        // covers everything, which is what #45 was about.
        assert!(
            checked > 0,
            "no document showed a sample of a command that changes nothing"
        );
    }
}

/// the command line, and reading the file it names.
mod the_command_line {
    use super::*;

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
}

/// the whole pipeline: source -> bytecode -> verifier -> VM.
mod the_pipeline {
    use super::*;

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
}

/// comparing two strings.
///
/// The VM has always been able to: `values_equal` has an arm for two strings,
/// the verifier's rule for `EQ` is about the two operands having the same type
/// rather than about which type, and `EQ` itself is three registers. The whole
/// of the refusal was one row of the operator table in the checker. These
/// tests are the end of the pipeline, so they say the row was the only thing
/// in the way.
mod comparing_strings {
    use super::*;

    /// Equality is byte equality of the interned string. Not case folding, not
    /// normalization, not trimming - so `"main" == "Main"` is false, and the
    /// program says so by weight rather than by returning a bare `1`, because
    /// a test that only asked whether two equal strings are equal would pass
    /// under case folding too.
    #[test]
    fn two_strings_compare_by_their_bytes() {
        let src = write_temp(
            "streq-bytes.sic",
            "fn bit(b: Bool, weight: Int) -> Int {\n\
             \x20   if b {\n\
             \x20       return weight;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let branch = \"main\";\n\
             \x20   return bit(branch == \"main\", 1)\n\
             \x20       + bit(branch == \"release\", 2)\n\
             \x20       + bit(branch != \"release\", 4)\n\
             \x20       + bit(branch == \"Main\", 8);\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        // 1: equal strings are equal. 4: `!=` is its negation. Not 2, because
        // different strings are not. Not 8, because `"main"` is not `"Main"`.
        assert_eq!(stdout, "5\n");
        std::fs::remove_file(src).ok();
    }

    /// `<` on strings needs a collation decision, and nothing has asked for
    /// one, so ordering stayed out. The note has to move with the operator
    /// table: "arithmetic and comparison on Int only" stopped being true of
    /// String the moment `==` started working.
    #[test]
    fn strings_have_no_ordering() {
        let src = write_temp(
            "streq-ordering.sic",
            "fn main() -> Int {\n\
             \x20   if \"a\" < \"b\" {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0303"), "{stderr}");
        assert!(
            stderr.contains("`<` cannot be applied to String"),
            "{stderr}"
        );
        assert!(
            stderr.contains("compares String with `==` and `!=` only"),
            "{stderr}"
        );
        std::fs::remove_file(src).ok();
    }

    /// The shape the change was made for: a program reads something and asks
    /// whether it is the one value it is allowed to act on. `fs.read` answers
    /// a plain `String`, so this is the whole path - source, checker, verifier,
    /// broker, VM - and not a literal compared with itself.
    #[test]
    fn a_program_can_ask_whether_a_file_says_the_expected_thing() {
        let data = write_temp("streq-branch.txt", "main");
        let data = data.to_str().unwrap().to_string();
        let src = write_temp(
            "streq-branch.sic",
            &format!(
                "allow {{ fs.read {data:?}; }}\n\
                 \n\
                 fn main() -> Int {{\n\
                 \x20   let branch = fs.read({data:?});\n\
                 \x20   if branch == \"main\" {{\n\
                 \x20       return 1;\n\
                 \x20   }}\n\
                 \x20   return 0;\n\
                 }}\n"
            ),
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert_eq!(stdout, "1\n");
        std::fs::remove_file(&data).ok();
        std::fs::remove_file(src).ok();
    }

    /// And the shape it was *argued* for, which it does not reach. The issue
    /// asks "did `git.rev_parse("HEAD")` resolve to the tag that was
    /// approved?" - but `git.rev_parse` answers `Observed<String>`, and a
    /// labelled value is refused as an operand whatever the operator table
    /// says. Equality on String was necessary for that question and is not
    /// sufficient; the same is true of asking whether a path is among what
    /// `git.status` reported, because indexing keeps the label too.
    #[test]
    fn what_a_repository_reported_is_still_not_an_operand() {
        let src = write_temp(
            "streq-revparse.sic",
            "allow {\n\
             \x20   git.rev_parse \"/usr/bin/git\" in \"/srv/thing\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let head = git.rev_parse(\"HEAD\");\n\
             \x20   if head == \"deadbeef\" {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0371"), "{stderr}");
        assert!(stderr.contains("Observed<String>"), "{stderr}");
        std::fs::remove_file(src).ok();
    }
}

/// nothing runs bytecode that does not verify.
///
/// `decode` establishes that a `Program` can exist and says nothing about
/// what is in it. A recorded program is a file with ordinary permissions, so
/// every path that picks one up again checks it - and each of these tests is
/// one of those paths.
mod verification {
    use super::*;

    /// Bytecode that decodes and does not verify, written where a recorded run
    /// keeps its program.
    ///
    /// An unknown opcode, because decoding a code section is reading `u32`s and
    /// says nothing about what they mean. That is the whole distinction this tests:
    /// `decode` establishes that a `Program` can exist, and the verifier is what
    /// establishes that it is safe to run.
    fn corrupt_program(dir: &std::path::Path) {
        let path = dir.join("program.sicb");
        let bytes = std::fs::read(&path).expect("the recorded program");
        let mut program = sic_bytecode::decode(&bytes).expect("it decoded when it was written");
        program.code[0] = sic_bytecode::inst::Inst(0xFFFF_FFFF);
        std::fs::write(&path, sic_bytecode::encode(&program)).expect("writable");
    }

    /// A recorded program is a file with ordinary permissions. The run that wrote
    /// it proves nothing about what is in it now, so every path that picks it up
    /// again checks it.
    #[test]
    fn nothing_runs_bytecode_that_does_not_verify() {
        let store = temp_dir("unverified-store");
        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("milestone.sic"), "--record"],
        );
        assert_eq!(code, 0, "{stderr}");
        let _ = stdout;

        let dir = std::fs::read_dir(&store)
            .expect("the store")
            .next()
            .expect("one run")
            .expect("readable")
            .path();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        corrupt_program(&dir);

        let (_, stderr, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
        assert_eq!(code, 1, "replay ran it anyway: {stderr}");
        assert!(stderr.contains("does not verify"), "{stderr}");
        assert!(stderr.contains("unknown opcode"), "{stderr}");

        std::fs::remove_dir_all(&store).ok();
    }

    /// The same door, on the path a waiting run comes back through.
    #[test]
    fn attach_will_not_pick_up_bytecode_that_does_not_verify() {
        let store = temp_dir("unverified-attach");
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("approval.sic"), "--record"],
        );
        assert_eq!(code, 3, "{stderr}");

        let dir = std::fs::read_dir(&store)
            .expect("the store")
            .next()
            .expect("one run")
            .expect("readable")
            .path();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        corrupt_program(&dir);

        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["attach", &id, "--value", "true"],
        );
        assert_eq!(code, 1, "attach ran it anyway: {stderr}");
        assert!(stderr.contains("does not verify"), "{stderr}");

        std::fs::remove_dir_all(&store).ok();
    }
}

/// capabilities.
mod capabilities {
    use super::*;

    /// A child depends on a directory and an environment whether or not the
    /// manifest mentions them. `in` is what makes the grant decide the first,
    /// so the same bytecode does the same thing from any shell.
    #[test]
    fn a_grant_can_say_which_directory_a_call_runs_in() {
        let src = write_temp(
            "cap-in.sic",
            "allow {\n\
             \x20   process.run \"/bin/pwd\" args [] in \"/tmp\";\n\
             }\n\
             \n\
             fn main() -> Observed<String> {\n\
             \x20   return process.run(\"/bin/pwd\", []).output;\n\
             }\n",
        );
        // From two different directories, the same answer. Without `in` this
        // is the test that would print two.
        for from in [repo_root(), std::path::PathBuf::from("/")] {
            let (stdout, stderr, code) = sic_in(from, &["run", src.to_str().unwrap()]);
            assert_eq!(code, 0, "stderr: {stderr}");
            assert_eq!(stdout.trim(), "\"/tmp\\n\"", "{stdout}");
        }
        std::fs::remove_file(src).ok();
    }

    /// The environment is cleared and then filled from the grant, so a child
    /// gets what the manifest says and nothing the shell happened to have.
    #[test]
    fn a_call_gets_the_environment_the_grant_names_and_no_other() {
        let src = write_temp(
            "cap-env.sic",
            "allow {\n\
             \x20   process.run \"/usr/bin/env\" args [] env { GREETING: \"from the manifest\" };\n\
             }\n\
             \n\
             fn main() -> Observed<String> {\n\
             \x20   return process.run(\"/usr/bin/env\", []).output;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("GREETING=from the manifest"), "{stdout}");
        // `SIC_RUNS` is set in this process by other tests and is never in the
        // child's, which is the half of this that matters.
        assert!(!stdout.contains("SIC_RUNS"), "{stdout}");
        assert!(!stdout.contains("PATH="), "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// A grant that says neither still depends on both, so the plan says which
    /// - a reader who is not told assumes the grant is the whole of it.
    #[test]
    fn a_plan_says_when_a_call_depends_on_the_shell_that_started_it() {
        let (stdout, _, code) = sic(&["plan", &example("run.sic")]);
        assert_eq!(code, 0);
        assert!(
            stdout.contains("in the directory `sic` is started in"),
            "{stdout}"
        );
        assert!(stdout.contains("with no environment"), "{stdout}");
    }

    /// `in` and `env` describe a child process, and a capability that starts
    /// none has nothing to do with either.
    #[test]
    fn only_a_capability_that_starts_a_process_takes_in_or_env() {
        let src = write_temp(
            "cap-in-wrong.sic",
            "allow {\n\
             \x20   fs.read \"./examples/greeting.txt\" in \"/tmp\";\n\
             }\n\
             fn main() -> String {\n\
             \x20   return fs.read(\"./examples/greeting.txt\");\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0334"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A relative `in` would be resolved against whatever shell started `sic`,
    /// which is the thing `in` exists to stop.
    #[test]
    fn a_relative_directory_is_refused_before_anything_runs() {
        let src = write_temp(
            "cap-in-relative.sic",
            "allow {\n\
             \x20   process.exec \"/bin/true\" args [] in \"./crates\";\n\
             }\n\
             fn main() -> Int {\n\
             \x20   return process.exec(\"/bin/true\", []);\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0335"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

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
}

/// the execution journal.
mod the_journal {
    use super::*;

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
}

/// suspend and resume.
mod suspend_and_resume {
    use super::*;

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

    /// A checkpoint belongs to the bytecode it was taken from, and editing the
    /// program orphans it. There is no `--force`; what there is now is a message
    /// that says which way is out, because a runtime that quietly accumulates
    /// unresumable state is not being safe, it is being unhelpful about being safe.
    #[test]
    fn a_checkpoint_whose_program_changed_says_what_to_do_next() {
        let store = temp_dir("orphaned");
        let src = store.join("waiting.sic");
        let program = std::fs::read_to_string(example("approval.sic")).expect("readable");
        std::fs::write(&src, &program).expect("writable");
        // Recorded, so the run keeps its own bytecode and its checkpoint beside it.
        // That is what the advice below rests on.
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 3, "{stderr}");

        let dir = std::fs::read_dir(&store)
            .expect("the store")
            .find_map(|e| {
                let path = e.ok()?.path();
                path.join("checkpoint.sicc").exists().then_some(path)
            })
            .expect("one recorded run");
        let checkpoint = dir.join("checkpoint.sicc");

        // An edit that changes the bytecode. The digest is over bytecode, so this
        // has to be a real change rather than a comment.
        std::fs::write(&src, program.replace("return", "return ")).expect("writable");
        std::fs::write(
            &src,
            std::fs::read_to_string(&src).unwrap().replace(
                "fn main()",
                "fn unused() -> Int { return 41; }\n\nfn main()",
            ),
        )
        .expect("writable");

        let (_, stderr, code) = sic(&[
            "resume",
            checkpoint.to_str().unwrap(),
            src.to_str().unwrap(),
            "--value",
            "true",
        ]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("cannot be resumed"), "{stderr}");
        // What it says to do about it.
        assert!(stderr.contains("sic attach"), "{stderr}");
        assert!(stderr.contains("kept its own bytecode"), "{stderr}");
        // And why an innocent edit is not what did it.
        assert!(stderr.contains("comment or a rename"), "{stderr}");

        // The recorded run really is still answerable, which is what that advice
        // rests on: it kept the bytecode it was compiled from.
        let id = dir.file_name().unwrap().to_str().unwrap().to_string();
        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["attach", &id, "--value", "true"],
        );
        assert_eq!(code, 0, "{stderr}");
        assert!(!stdout.is_empty(), "{stdout}");

        std::fs::remove_dir_all(&store).ok();
    }
}

/// tasks, retry and timeout.
mod tasks {
    use super::*;

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
            "allow { fs.read \"./x.txt\" repeatable; }\nfn main() -> String { return fs.read(\"./x.txt\") retry 3 timeout 250; }\n",
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
            "allow { fs.read \"./definitely-missing.txt\" repeatable; }\n\
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
}

/// records and lists.
mod records_and_lists {
    use super::*;

    /// `[]` has no element type of its own, and `E0342` is right that guessing
    /// one would move the error to wherever the list is used. It is not right
    /// where the answer is written down beside it, which is three places: a
    /// `let` annotation, a parameter, a return type.
    #[test]
    fn an_empty_list_takes_the_type_its_position_already_names() {
        let cases = [
            // A capability's parameter. This is the one #8 hit twice in an
            // afternoon: `process.run("/bin/pwd", [])`.
            (
                "empty-cap.sic",
                "allow {\n\
                 \x20   process.run \"/bin/pwd\" args [] in \"/tmp\";\n\
                 }\n\
                 fn main() -> Observed<String> {\n\
                 \x20   return process.run(\"/bin/pwd\", []).output;\n\
                 }\n",
                "\"/tmp\\n\"",
            ),
            // A function's parameter.
            (
                "empty-fn.sic",
                "fn count(xs: List<String>) -> Int { return len(xs); }\n\
                 fn main() -> Int { return count([]); }\n",
                "0",
            ),
            // A return type.
            (
                "empty-return.sic",
                "fn main() -> List<String> { return []; }\n",
                "[]",
            ),
            // The annotation, which has always worked and is here so that all
            // three of the places the rule applies are in one test.
            (
                "empty-let.sic",
                "fn main() -> Int {\n\
                 \x20   let xs: List<String> = [];\n\
                 \x20   return len(xs);\n\
                 }\n",
                "0",
            ),
        ];
        for (name, source, expected) in cases {
            let src = write_temp(name, source);
            let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
            assert_eq!(code, 0, "{name}: {stderr}");
            assert_eq!(stdout.trim(), expected, "{name}");
            std::fs::remove_file(src).ok();
        }
    }

    /// And still refused where nothing says. Guessing here would put the error
    /// wherever the list is used instead of where it was written.
    #[test]
    fn an_empty_list_with_nothing_to_take_a_type_from_is_still_refused() {
        let src = write_temp(
            "empty-nothing.sic",
            "fn main() -> Int {\n\
             \x20   let xs = [];\n\
             \x20   return len(xs);\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0342"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

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
}

/// `for` over a list.
///
/// The loop exists because iteration was spelled as recursion, and a recursion
/// costs a frame per element: `MAX_FRAMES` is 1024, so a walk over a list
/// longer than that failed at run time, having already done whatever it did
/// before reaching the element that broke it. A loop costs no frame at all, so
/// what bounds a walk becomes the run's own fuel. See issue #66.
mod loops {
    use super::*;

    /// A JSON array of `n` integers, and the program that walks it.
    ///
    /// `from_json` rather than a list literal: `MAKE_LIST` puts every element
    /// in a register of its own and there are 256 of them, so a literal cannot
    /// state a list of the size this is about.
    fn walk_of(n: usize, tail: &str) -> String {
        let mut doc = String::from("[");
        for i in 0..n {
            if i > 0 {
                doc.push(',');
            }
            doc.push_str(&i.to_string());
        }
        doc.push(']');
        format!(
            "fn main() -> Int {{\n\
             \x20   let xs: List<Int> = from_json(\"{doc}\");\n\
             \x20   {tail}\n\
             }}\n"
        )
    }

    /// The body runs once for each element, in order.
    ///
    /// What it says is the only thing there is to count: a loop cannot add
    /// anything up, because there is no assignment and so nothing in the body
    /// can carry a value to the next element.
    #[test]
    fn the_body_runs_once_for_each_element_in_order() {
        let src = write_temp(
            "for-order.sic",
            "fn main() -> Int {\n\
             \x20   let xs = [\"first\", \"second\", \"third\"];\n\
             \x20   for x in xs {\n\
             \x20       log warn x;\n\
             \x20   }\n\
             \x20   return len(xs);\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "3");
        let said: Vec<&str> = stderr.lines().filter(|l| l.starts_with("warn: ")).collect();
        assert_eq!(
            said,
            ["warn: first", "warn: second", "warn: third"],
            "{stderr}"
        );
        std::fs::remove_file(src).ok();
    }

    /// The whole motivation, and the failure it replaces.
    ///
    /// Two thousand elements is not an unusual number - `git.status()` on a
    /// dirty checkout of a large repository passes 1024 without trying. The
    /// same list folded by a recursion is the program this is for, and it is
    /// here so that what it does is a fact this test states rather than a claim
    /// the issue made.
    #[test]
    fn a_list_longer_than_the_call_stack_is_walked_to_the_end() {
        let looped = walk_of(
            2000,
            "for x in xs {\n\
             \x20       log info \"one\";\n\
             \x20   }\n\
             \x20   return len(xs);",
        );
        let src = write_temp("for-2000.sic", &looped);
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "2000");
        assert_eq!(stderr.lines().filter(|l| *l == "info: one").count(), 2000);
        std::fs::remove_file(src).ok();

        let recursed = format!(
            "fn total(xs: List<Int>, i: Int) -> Int {{\n\
             \x20   if i >= len(xs) {{ return 0; }}\n\
             \x20   return xs[i] + total(xs, i + 1);\n\
             }}\n{}",
            walk_of(2000, "return total(xs, 0);")
        );
        let src = write_temp("for-2000-recursed.sic", &recursed);
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("call stack too deep"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The count is `len(xs)`, taken once when the loop starts, so an empty
    /// list runs the body no times rather than once.
    #[test]
    fn an_empty_list_runs_the_body_no_times() {
        let src = write_temp(
            "for-empty.sic",
            "fn main() -> Int {\n\
             \x20   let xs: List<Int> = [];\n\
             \x20   for x in xs {\n\
             \x20       log error \"the body ran\";\n\
             \x20   }\n\
             \x20   return 7;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "7");
        assert!(!stderr.contains("the body ran"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The binding is scoped to the body, like a `let`, so the name is gone at
    /// the closing brace.
    #[test]
    fn the_loop_variable_does_not_outlive_the_body() {
        let src = write_temp(
            "for-scope.sic",
            "fn main() -> Int {\n\
             \x20   let xs = [1, 2];\n\
             \x20   for x in xs {\n\
             \x20       log info \"one\";\n\
             \x20   }\n\
             \x20   return x;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0300"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A capability call in a loop body runs once per element and appears in
    /// the plan, which already said it cannot bound how often a site runs. That
    /// sentence was written for a call behind an `if` and needed nothing added
    /// for a call inside a loop.
    #[test]
    fn a_capability_call_in_a_loop_body_runs_and_is_in_the_plan() {
        let src = write_temp(
            "for-cap.sic",
            "allow {\n\
             \x20   process.run \"/bin/echo\" args [];\n\
             }\n\
             fn main() -> Int {\n\
             \x20   let names = [\"a\", \"b\", \"c\"];\n\
             \x20   for name in names {\n\
             \x20       let r = process.run(\"/bin/echo\", []);\n\
             \x20       log info name;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            stderr.lines().filter(|l| l.starts_with("info: ")).count(),
            3
        );

        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("process.run"), "{stdout}");
        assert!(
            stdout.contains("depends on the path taken"),
            "the plan still says it cannot bound a site: {stdout}"
        );
        std::fs::remove_file(src).ok();
    }

    /// The bytecode a loop produces is a backward `JUMP`, and it verifies.
    ///
    /// `decode` then `verify` is what stands between a `.sicb` on a disk and
    /// the VM, so a loop going through `sic verify` here is the end-to-end half
    /// of the claim that the verifier needed nothing added. The other half is
    /// in `sic-verify`, over the instructions themselves.
    #[test]
    fn the_bytecode_a_loop_produces_verifies() {
        let src = write_temp(
            "for-verify.sic",
            "fn main() -> Int {\n\
             \x20   let xs = [1, 2, 3];\n\
             \x20   for x in xs {\n\
             \x20       log debug \"one\";\n\
             \x20   }\n\
             \x20   return len(xs);\n\
             }\n",
        );
        let out =
            std::env::temp_dir().join(format!("sic-test-{}-for-verify.sicb", std::process::id()));
        let (_, stderr, code) = sic(&[
            "compile",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        assert_eq!(code, 0, "{stderr}");

        let (stdout, stderr, code) = sic(&["verify", out.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("verified"), "{stdout}");
        // A loop that left a block nothing reaches behind would say so here.
        assert!(!stdout.contains("unreachable"), "{stdout}");
        assert!(!stderr.contains("unreachable"), "{stderr}");

        let (stdout, stderr, code) = sic(&["disasm", out.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            stdout
                .lines()
                .any(|l| l.contains("JUMP  ") && l.contains('-')),
            "a loop is a backward jump: {stdout}"
        );
        std::fs::remove_file(src).ok();
        std::fs::remove_file(out).ok();
    }

    /// Only a list can be walked. A `String` has a length, which makes it the
    /// case worth naming.
    #[test]
    fn walking_something_that_is_not_a_list_is_refused() {
        let src = write_temp(
            "for-not-a-list.sic",
            "fn main() -> Int {\n\
             \x20   for c in \"abc\" {\n\
             \x20       log info \"one\";\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0354"), "{stderr}");
        assert!(stderr.contains("cannot be walked"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// `while`, `loop` and `mut` are still reserved, and saying so here is what
    /// stops `for` from being read as "loops arrived". A `while` needs
    /// something to change between two visits to its condition, and nothing in
    /// this language changes.
    #[test]
    fn while_loop_and_mut_are_still_reserved() {
        for (name, word) in [
            ("for-kw-while.sic", "while"),
            ("for-kw-loop.sic", "loop"),
            ("for-kw-mut.sic", "mut"),
        ] {
            let src = write_temp(name, &format!("fn main() {{ let x = {word}; }}\n"));
            let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
            assert_eq!(code, 1, "{word}");
            assert!(stderr.contains("E0210"), "{word}: {stderr}");
            std::fs::remove_file(src).ok();
        }
    }
}

/// agents.
mod agents {
    use super::*;

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
}

/// exporting.
mod exporting {
    use super::*;

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
}

/// planning.
mod planning {
    use super::*;

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
        // Not "VERIFY Diagnosis": the column a verb sits in is formatting, and a
        // test that pins it fails every time a longer verb is added.
        assert!(stdout.contains("VERIFY"), "{stdout}");
        assert!(stdout.contains("Diagnosis"), "{stdout}");
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
}

/// the plan against the run.
///
/// `sic plan` says what a program may do, and the decision it exists for is made
/// before the run. A plan that names a capability too many is a nuisance; a plan
/// that misses one is a false statement made to whoever is deciding. Every other
/// plan test asserts what the plan says against what the test built, which cannot
/// catch a plan that misses something. These two compare it to a different
/// source: what a recorded run actually asked the broker for.
mod the_plan_against_the_run {
    use super::*;

    /// The verbs a plan leads a capability call with. `VERIFY`, `SPAWN` and `AWAIT`
    /// are steps too, and none of them reaches outside.
    const CAPABILITY_VERBS: &[&str] = &[
        "READ", "WRITE", "EXEC", "INVOKE", "CAPTURE", "RUN", "CHOOSE", "APPROVE",
    ];

    /// The capabilities a rendered plan names at a call site.
    ///
    /// Read from the steps rather than from the `Capabilities:` section, because the
    /// section is the manifest read back and the steps are what the plan's walker
    /// found in the code. A walker that skipped a `CALL_CAP` would still print the
    /// manifest entry for it, so a test that read the section would pass while the
    /// plan under-reported.
    fn capabilities_a_plan_names(stdout: &str) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        for line in stdout.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // `    1. READ     fs.read   "./examples/greeting.txt"   ; 12:12`
            let [index, verb, name, ..] = fields.as_slice() else {
                continue;
            };
            if index.ends_with('.') && CAPABILITY_VERBS.contains(verb) {
                names.insert((*name).to_string());
            }
        }
        names
    }

    /// The capabilities a recorded run asked for, from the run's own journal.
    ///
    /// `CapabilityRequested` is emitted where the VM suspends to have a call
    /// performed, so this is what the program reached for, not what it was allowed
    /// to reach for.
    fn capabilities_a_run_requested(store: &std::path::Path) -> std::collections::BTreeSet<String> {
        let dir = std::fs::read_dir(store)
            .expect("the run store should exist")
            .flatten()
            .next()
            .expect("the run should have been recorded")
            .path();
        let journal =
            std::fs::read_to_string(dir.join("journal.jsonl")).expect("a recorded journal");

        let mut names = std::collections::BTreeSet::new();
        for line in journal.lines() {
            if !line.contains("\"event\":\"capability_requested\"") {
                continue;
            }
            let Some(at) = line.find("\"cap\":\"") else {
                continue;
            };
            let rest = &line[at + "\"cap\":\"".len()..];
            let Some(end) = rest.find('"') else { continue };
            names.insert(rest[..end].to_string());
        }
        names
    }

    /// Asserts the one direction that matters, and says which way it is.
    fn the_plan_does_not_under_report(
        planned: &std::collections::BTreeSet<String>,
        requested: &std::collections::BTreeSet<String>,
    ) {
        let missing: Vec<&String> = requested.difference(planned).collect();
        assert!(
            missing.is_empty(),
            "the run called {missing:?}, which the plan did not name. A plan may \
         over-report - a grant that is never spent is a warning, not a lie - \
         and may never under-report, because the decision it is read for is \
         made before the run. Planned: {planned:?}. Called: {requested:?}"
        );
    }

    #[test]
    fn a_plan_names_the_capabilities_a_run_reaches_behind_a_branch() {
        // A call inside an `if`, in a function other than `main`, is one of the two
        // shapes a plan that walked only the obvious path would miss.
        let store = temp_store("plan-branch");
        let target = write_temp("plan-run-target.txt", "");
        let target = target.to_str().unwrap().to_string();
        let src = write_temp(
            "plan-branch.sic",
            &format!(
                "allow {{\n\
             \x20   fs.read \"./examples/greeting.txt\";\n\
             \x20   fs.write {target:?};\n\
             }}\n\
             \n\
             fn pick(n: Int) -> String {{\n\
             \x20   if n > 0 {{\n\
             \x20       return fs.read(\"./examples/greeting.txt\");\n\
             \x20   }}\n\
             \x20   return \"nothing\";\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   let text = pick(1);\n\
             \x20   fs.write({target:?}, \"done\");\n\
             \x20   return len(text);\n\
             }}\n"
            ),
        );

        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        let planned = capabilities_a_plan_names(&stdout);

        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 0, "stderr: {stderr}");
        let requested = capabilities_a_run_requested(&store);

        // Both calls happen on the path this run takes, so an empty set would mean
        // the test proved nothing rather than that the plan was right.
        assert_eq!(
            requested,
            ["fs.read".to_string(), "fs.write".to_string()].into(),
            "the run did not reach what this program was written to reach"
        );
        the_plan_does_not_under_report(&planned, &requested);

        std::fs::remove_file(src).ok();
        std::fs::remove_file(target).ok();
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn a_plan_names_a_capability_a_run_reaches_through_an_imported_module() {
        // The other shape: the call is in a file the command line never names, and
        // the grant is in the file that is run. `examples/import.sic` is the
        // program this is true of, so the test plans and runs the real one.
        //
        // What this comparison cannot catch is worth writing down. A capability call
        // in an imported module is also where a real miscompile lives (#36), and a
        // program it hits never asks for the capability at all - so the journal is
        // empty, and an empty set is a subset of anything. Ground truth here is what
        // a run did, which says nothing about a call that was compiled away before
        // the run began.
        let store = temp_store("plan-import");

        let (stdout, stderr, code) = sic(&["plan", &example("import.sic")]);
        assert_eq!(code, 0, "stderr: {stderr}");
        let planned = capabilities_a_plan_names(&stdout);

        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("import.sic"), "--record"],
        );
        // The program runs `/bin/echo`, and a machine without one should say so
        // rather than skip quietly: a test that passes by not running is worse than
        // one that fails.
        assert_eq!(code, 0, "stderr: {stderr}");
        let requested = capabilities_a_run_requested(&store);

        assert_eq!(requested, ["process.exec".to_string()].into());
        the_plan_does_not_under_report(&planned, &requested);

        std::fs::remove_dir_all(store).ok();
    }

    /// The graph, read back: node ids to labels, and the edges between them.
    ///
    /// Parsed rather than matched against a fixed string. A test that asserted
    /// on the whole document would fail whenever a label was reworded, and
    /// what is under test is the shape, not the wording.
    fn graph_of(
        stdout: &str,
    ) -> (
        std::collections::HashMap<String, String>,
        Vec<(String, String)>,
    ) {
        let mut labels = std::collections::HashMap::new();
        let mut edges = Vec::new();
        for line in stdout.lines().map(str::trim) {
            if let Some((from, rest)) = line.split_once(" --> ") {
                edges.push((from.to_string(), rest.to_string()));
                continue;
            }
            if let Some((from, rest)) = line.split_once(" -. spawn .-> ") {
                edges.push((from.to_string(), rest.to_string()));
                continue;
            }
            // `f0(["main"])` and `c0["EXEC process.exec - /usr/bin/true"]`.
            let Some(open) = line.find('[') else { continue };
            let Some(close) = line.rfind(']') else {
                continue;
            };
            let id = line[..open].trim_end_matches('(').to_string();
            let label = line[open..close]
                .trim_start_matches(['[', '(', '"'])
                .trim_end_matches(['"', ')'])
                .to_string();
            labels.insert(id, label);
        }
        (labels, edges)
    }

    /// The program the issue was written about: three blocks in the list, and
    /// nothing in it says `main` reaches either of the other two.
    fn shape(named: &str) -> std::path::PathBuf {
        write_temp(
            named,
            "allow {\n\
         \x20   human.approve \"the deploy\";\n\
         \x20   process.exec \"/usr/bin/true\";\n\
         \x20   fs.read \"./examples/greeting.txt\";\n\
         }\n\
         \n\
         fn deploy() -> Int {\n\
         \x20   return process.exec(\"/usr/bin/true\");\n\
         }\n\
         \n\
         fn rollback() -> String {\n\
         \x20   return fs.read(\"./examples/greeting.txt\");\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   let approved = human.approve(\"the deploy\");\n\
         \x20   if approved {\n\
         \x20       return deploy();\n\
         \x20   }\n\
         \x20   return len(rollback());\n\
         }\n",
        )
    }

    /// The gap the issue demonstrated, closed. The list prints `main`,
    /// `deploy` and `rollback` side by side with nothing between them; the
    /// graph says which reaches which.
    #[test]
    fn a_graph_says_which_functions_reach_which() {
        let src = shape("plan-shape-edges.sic");
        let path = src.to_str().unwrap().to_string();

        let (list, stderr, code) = sic(&["plan", &path]);
        assert_eq!(code, 0, "{stderr}");
        assert!(list.contains("deploy"), "{list}");
        assert!(!list.contains("-->"), "the list has no edges to lose");

        let (drawn, stderr, code) = sic(&["plan", &path, "--graph"]);
        assert_eq!(code, 0, "{stderr}");
        let (labels, edges) = graph_of(&drawn);
        let id_of = |name: &str| -> String {
            labels
                .iter()
                .find(|(_, label)| label.as_str() == name)
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| panic!("no node for {name} in\n{drawn}"))
        };
        let main = id_of("main");
        for reached in ["deploy", "rollback"] {
            let to = id_of(reached);
            assert!(
                edges.contains(&(main.clone(), to.clone())),
                "nothing says main reaches {reached}:\n{drawn}"
            );
        }
        std::fs::remove_file(src).ok();
    }

    /// And it says so without saying it will happen. An arrow is much harder
    /// to qualify than a sentence, so the qualification is a node rather than
    /// a footnote under a picture the reader has already drawn conclusions
    /// from.
    #[test]
    fn a_graph_says_may_rather_than_will() {
        let src = shape("plan-shape-caption.sic");
        let (drawn, _, code) = sic(&["plan", src.to_str().unwrap(), "--graph"]);
        assert_eq!(code, 0);
        assert!(drawn.contains("may, not will"), "{drawn}");
        std::fs::remove_file(src).ok();
    }

    /// Every capability the run actually reached is reachable in the graph by
    /// following edges from `main`. This is `the_plan_does_not_under_report`
    /// again, over arrows instead of over lines: a graph that drew the nodes
    /// and lost the path would pass the first test and fail a reader.
    #[test]
    fn a_graph_reaches_every_capability_a_run_reaches() {
        let store = temp_store("plan-graph-run");
        let src = shape("plan-shape-run.sic");
        let path = src.to_str().unwrap().to_string();

        let (drawn, stderr, code) = sic(&["plan", &path, "--graph"]);
        assert_eq!(code, 0, "{stderr}");

        // Answering `true` takes the branch through `deploy`.
        let (_, stderr, code) =
            sic_with_store(repo_root(), Some(&store), &["run", &path, "--record"]);
        assert_eq!(code, 3, "{stderr}");
        let dir = std::fs::read_dir(&store)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let id = dir.file_name().unwrap().to_string_lossy()[..8].to_string();
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["attach", &id, "--value", "true"],
        );
        assert_eq!(code, 0, "{stderr}");
        let requested = capabilities_a_run_requested(&store);
        assert_eq!(
            requested,
            ["human.approve".to_string(), "process.exec".to_string()].into(),
            "the run did not reach what this program was written to reach"
        );

        let (labels, edges) = graph_of(&drawn);
        let main = labels
            .iter()
            .find(|(_, l)| l.as_str() == "main")
            .map(|(id, _)| id.clone())
            .expect("a node for main");
        // Everything an arrow leads to from `main`, transitively.
        let mut seen = vec![main];
        let mut i = 0;
        while i < seen.len() {
            let from = seen[i].clone();
            for (a, b) in &edges {
                if *a == from && !seen.contains(b) {
                    seen.push(b.clone());
                }
            }
            i += 1;
        }
        let reachable: Vec<&String> = seen.iter().filter_map(|id| labels.get(id)).collect();
        for cap in &requested {
            assert!(
                reachable.iter().any(|label| label.contains(cap.as_str())),
                "the run called {cap}, and no path from main in the graph reaches it. \
                 A graph is read for the same decision the list is, and may \
                 over-report but never under-report.\nReachable: {reachable:?}\n{drawn}"
            );
        }

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// `spawn` is a call that does not wait, and a graph that drew it as an
    /// ordinary call would be describing a different program.
    #[test]
    fn a_spawn_is_not_drawn_as_a_call() {
        let (drawn, stderr, code) = sic(&["plan", &example("tasks.sic"), "--graph"]);
        assert_eq!(code, 0, "{stderr}");
        assert!(drawn.contains("-. spawn .->"), "{drawn}");
    }

    /// A grant nothing calls is still a grant - `sic mcp` serves it to the
    /// agent answering for the run - so a reader of the picture is told what a
    /// reader of the list is told.
    #[test]
    fn a_grant_nothing_calls_is_drawn_as_one() {
        let src = write_temp(
            "plan-graph-unused.sic",
            "allow {\n\
         \x20   fs.read \"./examples/greeting.txt\";\n\
         \x20   fs.write \"./never.txt\";\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   return len(fs.read(\"./examples/greeting.txt\"));\n\
         }\n",
        );
        let (drawn, _, code) = sic(&["plan", src.to_str().unwrap(), "--graph"]);
        assert_eq!(code, 0);
        assert!(drawn.contains("granted, and never called"), "{drawn}");
        assert!(drawn.contains("./never.txt"), "{drawn}");
        std::fs::remove_file(src).ok();
    }

    /// A constraint is a string from the source and can hold anything. Mermaid
    /// ends a quoted label at the next `"`, so a program could otherwise
    /// decide how its own plan is drawn.
    #[test]
    fn a_constraint_cannot_end_its_own_label() {
        let src = write_temp(
            "plan-graph-quote.sic",
            "allow {\n\
         \x20   process.exec \"/usr/bin/true\\\"] evil[\\\"\";\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   return 0;\n\
         }\n",
        );
        let (drawn, stderr, code) = sic(&["plan", src.to_str().unwrap(), "--graph"]);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            drawn.contains("#quot;"),
            "the quote was not escaped:\n{drawn}"
        );
        // A label opens at the first quote and closes at the last, so a third
        // one anywhere on the line is a label that ended where the program
        // said rather than where the renderer did.
        for line in drawn.lines() {
            assert!(
                line.matches('"').count() % 2 == 0 && line.matches('"').count() <= 2,
                "a label ended early: {line}\n{drawn}"
            );
        }
        std::fs::remove_file(src).ok();
    }
}

/// `git`, and the reason it is a capability rather than a `process.run` grant.
/// `docs/design/git.md`.
mod git {
    use super::*;

    /// Where git is on this machine. A test that guessed a path would be
    /// testing the machine rather than the broker.
    fn git_binary() -> Option<String> {
        for path in ["/usr/bin/git", "/bin/git", "/usr/local/bin/git"] {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }
        None
    }

    /// A repository of its own, so a test never reads the one it is running
    /// in: what is dirty there depends on who is working.
    fn a_repository(named: &str) -> Option<(String, std::path::PathBuf)> {
        let git = git_binary()?;
        let dir = temp_store(named);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new(&git)
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .expect("git runs")
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-qm", "one"]);
        Some((git, dir))
    }

    /// The two calls, against a repository this test made. A clean tree has
    /// nothing to report, and `HEAD` resolves to something 40 characters long.
    #[test]
    fn a_program_can_ask_what_a_repository_is() {
        let Some((git, dir)) = a_repository("git-ask") else {
            panic!("no git on this machine, and this test needs one rather than a pass");
        };
        let src = write_temp(
            "git-ask.sic",
            &format!(
                "allow {{\n\
             \x20   git.status {git:?} in {:?};\n\
             \x20   git.rev_parse {git:?} in {:?};\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   let head = git.rev_parse(\"HEAD\");\n\
             \x20   log info head;\n\
             \x20   return len(git.status());\n\
             }}\n",
                dir.to_string_lossy(),
                dir.to_string_lossy()
            ),
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "0", "a clean tree has nothing to report");
        // The commit this repository was just given.
        let logged = stderr
            .lines()
            .find(|l| l.starts_with("info:"))
            .unwrap_or_default();
        let hash = logged.trim_start_matches("info:").trim();
        assert_eq!(hash.len(), 40, "{stderr}");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{stderr}");

        // And a dirty tree has one entry per path, which is what makes `len`
        // the question a workflow actually asks.
        std::fs::write(dir.join("b.txt"), "two\n").unwrap();
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout.trim(), "1", "{stderr}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    /// The claim the whole capability rests on. A repository is data that came
    /// from somewhere; its hooks are executables that came with it. A
    /// `process.run "/usr/bin/git"` grant cannot say this, and that is why
    /// `git.status` exists.
    #[test]
    fn a_hook_in_the_repository_does_not_run() {
        let Some((git, dir)) = a_repository("git-hook") else {
            panic!("no git on this machine, and this test needs one rather than a pass");
        };
        let ran = dir.join("the-hook-ran");
        let hooks = dir.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        // `post-index-change` is a read-path hook: `git status` refreshes the
        // index, so this is one git would run.
        let hook = hooks.join("post-index-change");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch {:?}\n", ran.to_string_lossy()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // And a config that names a program, which is the other half.
        std::fs::write(
            dir.join(".git/config"),
            format!(
                "[core]\n\trepositoryformatversion = 0\n\tpager = sh -c 'touch {:?}'\n",
                ran.to_string_lossy()
            ),
        )
        .unwrap();

        let src = write_temp(
            "git-hook.sic",
            &format!(
                "allow {{\n\
             \x20   git.status {git:?} in {:?};\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   return len(git.status());\n\
             }}\n",
                dir.to_string_lossy()
            ),
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            !ran.exists(),
            "the repository's own hook or pager ran, which is the one thing this \
             capability exists to prevent"
        );

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    /// A revision is a name. One that starts with `-` is an option, and an
    /// option is how a read becomes something else.
    #[test]
    fn a_revision_that_is_an_option_is_refused() {
        let Some((git, dir)) = a_repository("git-option") else {
            panic!("no git on this machine, and this test needs one rather than a pass");
        };
        let src = write_temp(
            "git-option.sic",
            &format!(
                "allow {{\n\
             \x20   git.rev_parse {git:?} in {:?};\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   return len(git.rev_parse(\"--git-dir\"));\n\
             }}\n",
                dir.to_string_lossy()
            ),
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("is not a revision"), "{stderr}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    /// What git reads is what this capability decides, so a manifest cannot
    /// take the decision back. Refused at compile time rather than ignored at
    /// the call.
    #[test]
    fn a_git_grant_cannot_say_what_git_reads() {
        let src = write_temp(
            "git-env.sic",
            "allow {\n\
         \x20   git.status \"/usr/bin/git\" in \"/tmp\" env { GIT_CONFIG_GLOBAL: \"/tmp/mine\" };\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   return len(git.status());\n\
         }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0336"), "{stderr}");
        assert!(stderr.contains("decides its own environment"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// It runs a program and reads the answer, which is neither `READ` - a
    /// file - nor `EXEC` - a program whose arguments the program chose. And
    /// the plan says which repository, because that is what a reader is being
    /// asked to allow.
    #[test]
    fn a_plan_says_which_repository_and_what_git_may_read() {
        let src = write_temp(
            "git-plan.sic",
            "allow {\n\
         \x20   git.status \"/usr/bin/git\" in \"/srv/thing\";\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   return len(git.status());\n\
         }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("INSPECT"), "{stdout}");
        assert!(stdout.contains("in \"/srv/thing\""), "{stdout}");
        assert!(
            stdout.contains("reading no configuration but this repository's"),
            "{stdout}"
        );
        // Not "with no environment", which would read as a thing this grant
        // chose rather than one it cannot change.
        assert!(!stdout.contains("with no environment"), "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// The output is what a program printed, so it cannot decide what another
    /// program is told - which is the whole of what `Observed` is for.
    #[test]
    fn what_git_said_cannot_decide_what_runs() {
        let src = write_temp(
            "git-trust.sic",
            "allow {\n\
         \x20   git.rev_parse \"/usr/bin/git\" in \"/srv/thing\";\n\
         \x20   fs.write \"./out.txt\";\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   fs.write(\"./out.txt\", git.rev_parse(\"HEAD\"));\n\
         \x20   return 0;\n\
         }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0372"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The safe command line is the broker's, and no rule about a shell
    /// command can hold it - so the agent reaches git only by coming back
    /// through the broker, where the grant is applied again.
    ///
    /// Without `delegable`, which a `git` grant cannot say (E0329) and does
    /// not need: that word means something only where the manifest has not
    /// already bounded the authority, and this one is bounded three times -
    /// which binary, which repository, and a command line the agent never gets
    /// to write.
    #[test]
    fn the_agent_reaches_git_only_through_the_broker() {
        let src = write_temp(
            "git-agent.sic",
            // With a model call, because that is when there is an agent for
            // the plan to say anything about.
            "allow {\n\
         \x20   git.status \"/usr/bin/git\" in \"/srv/thing\";\n\
         \x20   llm.invoke \"claude\";\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   let said = llm.invoke(\"hello\");\n\
         \x20   return len(said) + len(git.status());\n\
         }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("through the broker"), "{stdout}");
        // Which capability, not only which binary. Two `git` grants on one
        // repository name the same constraint, so a line that printed only
        // that would say the same thing twice and tell a reader neither what
        // the agent may do nor that there were two of them.
        assert!(stdout.contains("git.status in \"/srv/thing\""), "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// And the word is refused rather than ignored, so a manifest cannot look
    /// like it widened something it did not.
    #[test]
    fn a_git_grant_cannot_say_delegable() {
        let src = write_temp(
            "git-delegable.sic",
            "allow {\n\
         \x20   git.status \"/usr/bin/git\" in \"/srv/thing\" delegable;\n\
         }\n\
         \n\
         fn main() -> Int {\n\
         \x20   return len(git.status());\n\
         }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0329"), "{stderr}");
        std::fs::remove_file(src).ok();
    }
}

/// What every capability does about the things a grant may say.
///
/// This is `every_diagnostic_code_is_in_the_index` for capabilities. Each of
/// the facts below is decided somewhere else - `in` and `env` in the type
/// checker, `delegable` beside them, how an agent reaches it in `sic-core` -
/// and each is decided by testing a prefix of the capability's name, in nine
/// places across three crates. That is #63.
///
/// This table does not make #63 unnecessary: adding a capability still means
/// visiting those nine places and getting each right. What it does is make
/// getting one wrong *loud*. A capability with no row here fails the first
/// test; a row that disagrees with what the binary actually does fails the
/// rest. Both of the mistakes #62 made would have failed here rather than
/// being found by reading the output.
mod what_a_grant_may_say {
    use super::*;

    /// How the agent answering this program's model calls reaches a grant.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Reaches {
        /// Translated into the agent's own permissions.
        ItsOwnTool,
        /// Back through the broker, against this manifest.
        TheBroker,
        /// Not at all.
        Not,
        /// It is the grant being exercised: answering it is what the agent is
        /// for, so a plan says nothing about it as a tool.
        ItIsTheAgent,
    }

    struct Row {
        cap: &'static str,
        /// Whether the grant may say `in "/abs"`, which only a capability that
        /// starts a process has any use for.
        takes_in: bool,
        /// Whether it may say `env { … }`. `git` may not: what git reads is
        /// the decision that capability exists to take (E0336).
        takes_env: bool,
        /// Whether `delegable` means anything. It does only where the manifest
        /// has not already bounded the authority (E0329).
        takes_delegable: bool,
        reaches: Reaches,
    }

    const TABLE: &[Row] = &[
        Row {
            cap: "fs.read",
            takes_in: false,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::ItsOwnTool,
        },
        Row {
            cap: "fs.write",
            takes_in: false,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::ItsOwnTool,
        },
        Row {
            cap: "llm.invoke",
            takes_in: false,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::ItIsTheAgent,
        },
        Row {
            cap: "human.approve",
            takes_in: false,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::TheBroker,
        },
        Row {
            cap: "human.choose",
            takes_in: false,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::TheBroker,
        },
        Row {
            cap: "git.status",
            takes_in: true,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::TheBroker,
        },
        Row {
            cap: "git.rev_parse",
            takes_in: true,
            takes_env: false,
            takes_delegable: false,
            reaches: Reaches::TheBroker,
        },
        Row {
            cap: "process.capture",
            takes_in: true,
            takes_env: true,
            takes_delegable: true,
            reaches: Reaches::Not,
        },
        Row {
            cap: "process.run",
            takes_in: true,
            takes_env: true,
            takes_delegable: true,
            reaches: Reaches::Not,
        },
        Row {
            cap: "process.exec",
            takes_in: true,
            takes_env: true,
            takes_delegable: true,
            reaches: Reaches::Not,
        },
    ];

    /// A program that grants `cap` and says one extra thing about it, and does
    /// nothing else. An unused grant is a warning rather than an error, so
    /// whether this compiles is exactly whether the extra thing was allowed.
    fn granting(cap: &str, named: &str, extra: &str) -> std::path::PathBuf {
        write_temp(
            // Named for what is being asked rather than for the text asking
            // it: `write_temp` keys on the process id, and these tests run at
            // the same time as each other.
            &format!("cap-{}-{named}.sic", cap.replace('.', "-")),
            &format!(
                "allow {{\n\
             \x20   {cap} \"/usr/bin/true\"{extra};\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   return 0;\n\
             }}\n"
            ),
        )
    }

    fn compiles(cap: &str, named: &str, extra: &str) -> (bool, String) {
        let src = granting(cap, named, extra);
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        std::fs::remove_file(src).ok();
        (code == 0, stderr)
    }

    /// The first thing, and the reason the rest is worth anything: a
    /// capability that was added without a row here has not been thought about
    /// in this file, and probably not in the other nine either.
    #[test]
    fn every_capability_has_a_row() {
        let known: Vec<&str> = TABLE.iter().map(|r| r.cap).collect();
        let mut missing = Vec::new();
        for name in sic_types::cap::all_names() {
            if !known.contains(&name) {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "these capabilities exist and this table does not say what a grant on them \
             may say: {missing:?}. Adding a capability means deciding, in nine places \
             across three crates, whether it takes `in`, `env` and `delegable` and how \
             an agent reaches it - see #63. Say so here too, and the tests below check \
             that what you said is what the binary does."
        );
        // And the other direction, so a removed capability does not leave a
        // row that quietly tests nothing.
        for row in TABLE {
            assert!(
                sic_types::cap::all_names().contains(&row.cap),
                "`{}` has a row and is not a capability",
                row.cap
            );
        }
    }

    /// `in` names a directory, which means something only to a capability that
    /// starts a process (E0334).
    #[test]
    fn in_is_taken_by_exactly_the_capabilities_that_start_a_process() {
        for row in TABLE {
            let (ok, stderr) = compiles(row.cap, "in", " in \"/tmp\"");
            assert_eq!(
                ok, row.takes_in,
                "`{}` and `in`: the table says {}, the binary says {}\n{stderr}",
                row.cap, row.takes_in, ok
            );
            if !ok {
                assert!(stderr.contains("E0334"), "{}: {stderr}", row.cap);
            }
        }
    }

    /// `env` says what a child is given, and a `git` grant may not say it:
    /// what git reads is the decision that capability exists to take.
    #[test]
    fn env_is_taken_by_fewer_than_in_is() {
        for row in TABLE {
            let (ok, stderr) = compiles(row.cap, "env", " env { A: \"b\" }");
            assert_eq!(
                ok, row.takes_env,
                "`{}` and `env`: the table says {}, the binary says {}\n{stderr}",
                row.cap, row.takes_env, ok
            );
            // Two different refusals, and which one it is matters: one says
            // there is no child to give an environment to, the other says
            // there is and this grant does not get to describe it.
            if !ok {
                let expected = match row.takes_in {
                    true => "E0336",
                    false => "E0334",
                };
                assert!(stderr.contains(expected), "{}: {stderr}", row.cap);
            }
        }
    }

    /// `delegable` means something only where the manifest has not already
    /// bounded the authority (E0329).
    #[test]
    fn delegable_is_taken_by_exactly_the_grants_it_could_widen() {
        for row in TABLE {
            let (ok, stderr) = compiles(row.cap, "delegable", " delegable");
            assert_eq!(
                ok, row.takes_delegable,
                "`{}` and `delegable`: the table says {}, the binary says {}\n{stderr}",
                row.cap, row.takes_delegable, ok
            );
            if !ok {
                assert!(stderr.contains("E0329"), "{}: {stderr}", row.cap);
            }
        }
    }

    /// And how the agent reaches it, which `sic plan` prints and a person
    /// deciding whether to run this reads.
    #[test]
    fn a_plan_says_how_the_agent_reaches_every_grant() {
        for row in TABLE {
            if row.reaches == Reaches::ItIsTheAgent {
                continue;
            }
            // With a model call, because that is when there is an agent for
            // the plan to say anything about.
            let src = write_temp(
                &format!("cap-reach-{}.sic", row.cap.replace('.', "-")),
                &format!(
                    "allow {{\n\
                 \x20   {} \"/usr/bin/true\"{};\n\
                 \x20   llm.invoke \"m\";\n\
                 }}\n\
                 \n\
                 fn main() -> LLM<String> {{\n\
                 \x20   return llm.invoke(\"hello\");\n\
                 }}\n",
                    row.cap,
                    // No `delegable`: what is under test is where a grant
                    // reaches the agent by default.
                    ""
                ),
            );
            let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
            assert_eq!(code, 0, "{}: {stderr}", row.cap);
            let said = match row.reaches {
                Reaches::ItsOwnTool => "its own permissions",
                Reaches::TheBroker => "through the broker",
                Reaches::Not => "the agent may not  use",
                Reaches::ItIsTheAgent => unreachable!("skipped above"),
            };
            assert!(
                stdout.contains(said),
                "`{}` should reach the agent as {:?}, and the plan does not say so:\n{stdout}",
                row.cap,
                row.reaches
            );
            std::fs::remove_file(src).ok();
        }
    }
}

/// Every field of a grant survives the journey from bytecode to a plan.
///
/// A grant is declared three times - `CapDecl` in `sic-bytecode`, `CapGrant`
/// in `sic-core`, `Grant` in `sic-plan` - and copied between them field by
/// field, by hand, in three places. That is #64, and the reason it is a
/// problem is not that three structs agree: it is that several of the fields
/// are `String`, so writing `dir: c.constraints.clone()` compiles.
///
/// What fails then is `sic plan`, quietly, by printing the wrong thing or
/// nothing - and `sic plan` is the one output in this project that must never
/// under-report, because it is what a person reads to decide whether to run a
/// program at all.
///
/// So every field gets a value that could not have come from any other field,
/// and the plan has to name each one where it belongs.
mod nothing_is_lost_in_transcription {
    use super::*;

    /// A digest is 64 hex characters and nothing here runs, so this is never
    /// checked against a file - it only has to be recognisable.
    const PIN: &str = "abc0000000000000000000000000000000000000000000000000000000000def";

    /// Named by the caller, because `write_temp` keys on the process id and
    /// these two tests run at the same time as each other.
    fn a_grant_that_says_everything(named: &str) -> std::path::PathBuf {
        write_temp(
            named,
            &format!(
                "allow {{\n\
             \x20   process.run \"/usr/bin/theconstraint\"\n\
             \x20       args [\"theargument\"]\n\
             \x20       sha256 \"{PIN}\"\n\
             \x20       in \"/thedirectory\"\n\
             \x20       env {{ THEVARIABLE: \"thevalue\" }}\n\
             \x20       repeatable delegable;\n\
             \x20   llm.invoke \"themodel\";\n\
             }}\n\
             \n\
             fn main() -> Int {{\n\
             \x20   let said = llm.invoke(\"hello\");\n\
             \x20   let r = process.run(\"/usr/bin/theconstraint\", [\"theargument\"]);\n\
             \x20   return r.code + len(said);\n\
             }}\n"
            ),
        )
    }

    /// Each field, named where it belongs. A value in the wrong place fails
    /// twice over - the line it should be on is missing it, and the line it
    /// landed on says something that was never granted.
    #[test]
    fn a_plan_names_every_field_of_a_grant_where_it_belongs() {
        let src = a_grant_that_says_everything("grant-every-field.sic");
        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");

        let wanted: &[(&str, &str)] = &[
            ("the constraint", "\"/usr/bin/theconstraint\""),
            ("the argument prefix", "args [\"theargument\"]"),
            ("the digest pin", &format!("sha256:{PIN}")),
            ("the directory", "in \"/thedirectory\""),
            ("the environment", "env THEVARIABLE"),
            ("repeatable", "repeatable"),
            ("delegable", "delegable"),
            ("the model", "\"themodel\""),
        ];
        for (what, text) in wanted {
            assert!(
                stdout.contains(text),
                "{what} did not survive the journey from bytecode to the plan: \
                 nothing in the output says {text:?}. A grant is transcribed field by \
                 field in three places (#64), and several of its fields are `String`, \
                 so a line that copies the wrong one compiles.\n{stdout}"
            );
        }

        // And nowhere it does not belong. `dir` and `constraint` are both
        // `String`, so the copy that swaps them is the one that compiles - and
        // this is what catches it.
        assert!(
            !stdout.contains("in \"/usr/bin/theconstraint\""),
            "the constraint was printed as the directory:\n{stdout}"
        );
        assert!(
            !stdout.contains("\"/thedirectory\"  ("),
            "the directory was printed as the constraint:\n{stdout}"
        );

        std::fs::remove_file(src).ok();
    }

    /// The same journey again, one crate further: what the agent may do is
    /// worked out from a `CapGrant` copied out of the plan's own `Grant`, so a
    /// field lost there is a plan that describes a different agent.
    #[test]
    fn the_agent_section_is_made_from_the_same_grant() {
        let src = a_grant_that_says_everything("grant-every-field-agent.sic");
        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");

        let line = stdout
            .lines()
            .find(|l| l.contains("the agent may use"))
            .unwrap_or_else(|| panic!("a delegable grant reaches the agent:\n{stdout}"));
        // The name it may use, and the digest that names which one - both of
        // which had to cross into `CapGrant` to be here at all.
        assert!(line.contains("/usr/bin/theconstraint"), "{line}");
        assert!(line.contains(&PIN[..8]), "the pin did not cross: {line}");

        std::fs::remove_file(src).ok();
    }
}

/// trust and provenance.
mod trust {
    use super::*;

    /// A label leaves a value only through `approve` - which is what E0371 is
    /// for, and it is not a rule about branching. `docs/design/trust.md` §2a.
    #[test]
    fn an_operator_cannot_take_a_label_off_a_value() {
        let src = write_temp(
            "trust-operand.sic",
            "type Diagnosis { cause: String, severity: Int }\n\
             \n\
             allow {\n\
             \x20   llm.invoke \"m\";\n\
             }\n\
             \n\
             agent diagnose { input: String, output: Diagnosis, budget: 1 }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let d = diagnose(\"why?\");\n\
             \x20   if d.severity > 5 {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0371"), "{stderr}");
        // Reading a field kept the label, which is why the operand is
        // `LLM<Int>` rather than `Int`.
        assert!(stderr.contains("LLM<Int>"), "{stderr}");
        // And the refusal names the door through.
        assert!(stderr.contains("approve("), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A person's choice is labelled too, so the same rule catches it. Named
    /// separately because the two labels are produced by different things and
    /// a rule that only covered one would be about spelling.
    #[test]
    fn a_persons_choice_is_not_an_operand_either() {
        let src = write_temp(
            "trust-chosen.sic",
            "allow {\n\
             \x20   human.choose \"which channel\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let c = choose(\"which?\", [\"a\", \"b\"]);\n\
             \x20   if c == 0 {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0371"), "{stderr}");
        assert!(stderr.contains("HumanChosen"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// `approve` changes the label rather than removing it: `HumanApproved<T>`
    /// is refused as an operand like any other. What a person's approval buys
    /// is reach (the value may now go to a capability that changes something),
    /// not arithmetic. The first draft of `trust.md` §2a said "a label leaves
    /// a value only through `approve`", and this test is why that sentence is
    /// no longer there.
    #[test]
    fn an_approval_buys_reach_not_arithmetic() {
        let src = write_temp(
            "trust-approved-operand.sic",
            "type Diagnosis { cause: String, severity: Int }\n\
             \n\
             allow {\n\
             \x20   llm.invoke \"m\";\n\
             \x20   human.approve \"use it\";\n\
             }\n\
             \n\
             agent diagnose { input: String, output: Diagnosis, budget: 1 }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let d = diagnose(\"why?\");\n\
             \x20   let ok = approve(\"use this?\", d);\n\
             \x20   if ok.severity > 5 {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0371"), "{stderr}");
        assert!(stderr.contains("HumanApproved<Int>"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// `len` takes a labelled value and answers a plain `Int`, so a model can
    /// steer a branch by the length of its own answer. That is deliberate, and
    /// `docs/design/trust.md` §2a argues it: a branch is not an effect, the
    /// manifest is the unit of approval, and nobody can get the answer back
    /// out of its length.
    ///
    /// **This test exists so that changing it is a decision rather than a
    /// regression.** Before §2a the behaviour was one sentence in `check_len`
    /// and nothing in a test, and an edit that made the label propagate would
    /// have looked like a fix.
    #[test]
    fn len_takes_the_label_off_and_that_is_on_purpose() {
        let src = write_temp(
            "trust-len.sic",
            "allow {\n\
             \x20   llm.invoke \"m\";\n\
             \x20   process.exec \"/bin/echo\" args [\"deploying\"];\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let said = llm.invoke(\"a long word to deploy, a short one not to\");\n\
             \x20   if len(said) > 5 {\n\
             \x20       return process.exec(\"/bin/echo\", [\"deploying\"]);\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(
            code, 0,
            "`len` of a model's answer is a plain Int on purpose - see \
             docs/design/trust.md §2a before changing this\n{stderr}"
        );
        // And the effect it steers into is in the manifest, which is the
        // reason the channel is accepted: a reader approved this program's
        // being able to run it.
        assert!(stdout.contains("process.exec"), "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// The same for `Observed`, and this one is load-bearing: `git.status()`
    /// answers `Observed<List<String>>`, and counting it is the question that
    /// capability exists to answer.
    #[test]
    fn what_a_program_printed_can_be_counted() {
        let src = write_temp(
            "trust-observed-len.sic",
            "allow {\n\
             \x20   process.capture \"/bin/echo\" args [\"hi\"];\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let said = process.capture(\"/bin/echo\", [\"hi\"]);\n\
             \x20   if len(said) > 0 {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        // What the program returned, which is not the process's exit code:
        // `/bin/echo hi` printed something, so the length was above zero.
        assert_eq!(stdout.trim(), "1", "{stderr}");
        std::fs::remove_file(src).ok();
    }

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

    /// Equality on String did not open a door for a model's answer, and the
    /// reason is structural rather than a second rule: `check_binary` rejects
    /// a labelled operand *before* it consults the operator table, so the
    /// refusal is E0371 - the laundering rule - and not E0303. It would still
    /// be E0371 if every operator in the language accepted String.
    #[test]
    fn what_an_agent_answered_cannot_be_compared_with_a_string() {
        let src = write_temp(
            "trust-eq-agent.sic",
            "allow {\n\
             \x20   llm.invoke \"m\";\n\
             }\n\
             \n\
             agent ask { input: String, output: String, budget: 1 }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let said = ask(\"ship it?\");\n\
             \x20   if said == \"yes\" {\n\
             \x20       return 1;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        // E0371, not E0303: the operand was refused for where it came from,
        // not for which operator was applied to it.
        assert!(stderr.contains("E0371"), "{stderr}");
        assert!(!stderr.contains("E0303"), "{stderr}");
        assert!(stderr.contains("LLM<String>"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The hole #72 was about, closed: a *direct* `llm.invoke` is labelled at
    /// the capability table, so both spellings of asking a model are checked
    /// the same way. Before this, the label was attached where an `agent` call
    /// is checked, and the lower-level door was exempt from the rule
    /// `trust.md` §2 opens with.
    #[test]
    fn a_direct_model_call_is_labelled_like_any_other() {
        let src = write_temp(
            "trust-direct-invoke.sic",
            "allow {\n\
             \x20   llm.invoke \"m\";\n\
             \x20   fs.write \"./out.txt\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   fs.write(\"./out.txt\", llm.invoke(\"say something\"));\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0372"), "{stderr}");
        assert!(stderr.contains("LLM<String>"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// And the two spellings agree. An `agent` declaration is `from_json` over
    /// a model call with a shape declared for the answer; written out by hand
    /// it has to reach the same refusal, or the label is about which syntax
    /// somebody used.
    #[test]
    fn the_manual_spelling_of_an_agent_carries_the_same_label() {
        let src = write_temp(
            "trust-manual-agent.sic",
            "type D { cause: String }\n\
             \n\
             allow {\n\
             \x20   llm.invoke \"m\";\n\
             \x20   fs.write \"./out.txt\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let d: LLM<D> = from_json(llm.invoke(\"why?\"));\n\
             \x20   fs.write(\"./out.txt\", d.cause);\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0372"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// `from_json` reads a document into a shape; it does not decide where the
    /// document came from. A plain `String` in still answers a plain record,
    /// so nothing that was not talking to a model gained a label.
    #[test]
    fn reading_a_plain_document_answers_a_plain_record() {
        let src = write_temp(
            "trust-plain-json.sic",
            "type D { cause: String }\n\
             \n\
             allow {\n\
             \x20   fs.write \"./out.txt\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let d: D = from_json(\"{\\\"cause\\\":\\\"x\\\"}\");\n\
             \x20   fs.write(\"./out.txt\", d.cause);\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        std::fs::remove_file(src).ok();
    }
}

/// recorded runs.
mod recorded_runs {
    use super::*;

    /// A program with one `fs.read`.
    ///
    /// `sic recheck` compiles a source file, so the grant has to name a path
    /// that exists from the repository root, which is where these run.
    const READER: &str = "allow {\n\
                          \x20   fs.read \"./examples/greeting.txt\";\n\
                          }\n\
                          \n\
                          fn main() -> String {\n\
                          \x20   return fs.read(\"./examples/greeting.txt\");\n\
                          }\n";

    /// Records one run of `src` and returns its id.
    fn recorded(store: &std::path::Path, src: &std::path::Path) -> String {
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert!(code == 0 || code == 3, "stderr: {stderr}");
        let (stdout, _, _) = sic_with_store(repo_root(), Some(store), &["runs"]);
        stdout
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next())
            .expect("one recorded run")
            .to_string()
    }

    /// The same program the run was recorded from still asks what it asked.
    ///
    /// The bytecode is not the same - the file is compiled again - so this is
    /// the claim `recheck` makes and `replay` does not: the recorded answers
    /// still fit.
    #[test]
    fn an_unchanged_program_still_asks_what_the_recording_answered() {
        let store = temp_store("recheck-same");
        let src = write_temp("recheck-same.sic", READER);
        let id = recorded(&store, &src);

        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, src.to_str().unwrap()],
        );
        assert_eq!(code, 0, "stderr: {stderr}\n{stdout}");
        assert!(stdout.contains("1 of 1 calls matched"), "{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// An edit that changes no call is not a difference. Bytecode digests move
    /// for a comment; the question the recording answered does not.
    #[test]
    fn an_edit_that_changes_no_call_is_not_a_difference() {
        let store = temp_store("recheck-comment");
        let src = write_temp("recheck-comment.sic", READER);
        let id = recorded(&store, &src);

        let edited = write_temp(
            "recheck-comment-edited.sic",
            &format!("// a comment nobody ran\n{READER}"),
        );
        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, edited.to_str().unwrap()],
        );
        assert_eq!(code, 0, "stderr: {stderr}\n{stdout}");
        assert!(stdout.contains("1 of 1 calls matched"), "{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_file(edited).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// The recorded answer would be handed to a different question, so it is a
    /// finding and the message says which call.
    #[test]
    fn a_program_that_asks_something_else_says_which_call() {
        let store = temp_store("recheck-other");
        let src = write_temp("recheck-other.sic", READER);
        let id = recorded(&store, &src);

        let other = write_temp(
            "recheck-other-cap.sic",
            "allow {\n\
             \x20   process.exec \"/usr/bin/true\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   return process.exec(\"/usr/bin/true\");\n\
             }\n",
        );
        let (stdout, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, other.to_str().unwrap()],
        );
        assert_eq!(code, 1, "{stdout}");
        assert!(
            stdout.contains(
                "call 1: the recording answered `fs.read`, this program asks `process.exec`"
            ),
            "{stdout}"
        );

        std::fs::remove_file(src).ok();
        std::fs::remove_file(other).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// The same capability with different arguments is a different question,
    /// which is why the comparison is on the argument digest and not the name.
    #[test]
    fn the_same_capability_with_other_arguments_is_a_different_question() {
        let store = temp_store("recheck-args");
        let src = write_temp("recheck-args.sic", READER);
        let id = recorded(&store, &src);

        let other = write_temp(
            "recheck-args-other.sic",
            "allow {\n\
             \x20   fs.read \"./examples/milestone.sic\";\n\
             }\n\
             \n\
             fn main() -> String {\n\
             \x20   return fs.read(\"./examples/milestone.sic\");\n\
             }\n",
        );
        let (stdout, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, other.to_str().unwrap()],
        );
        assert_eq!(code, 1, "{stdout}");
        assert!(
            stdout.contains("with different arguments than the recording answered"),
            "{stdout}"
        );

        std::fs::remove_file(src).ok();
        std::fs::remove_file(other).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// A program that no longer reaches the call the recording made.
    #[test]
    fn a_program_that_makes_fewer_calls_is_a_finding() {
        let store = temp_store("recheck-fewer");
        let src = write_temp("recheck-fewer.sic", READER);
        let id = recorded(&store, &src);

        let other = write_temp("recheck-none.sic", "fn main() -> Int { return 1; }\n");
        let (stdout, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, other.to_str().unwrap()],
        );
        assert_eq!(code, 1, "{stdout}");
        assert!(
            stdout.contains("no longer goes where the run went"),
            "{stdout}"
        );

        std::fs::remove_file(src).ok();
        std::fs::remove_file(other).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// A program with one budgeted site, so that the charge has an obvious
    /// call to belong to.
    const BUDGETED: &str = "type D { cause: String }\n\
                            allow {\n\
                            \x20   process.run \"/bin/echo\" args [];\n\
                            \x20   llm.invoke \"claude-opus-4\";\n\
                            }\n\
                            agent diagnose { input: String, output: D, budget: 2 }\n\
                            fn main() -> LLM<String> {\n\
                            \x20   let r = process.run(\"/bin/echo\", []);\n\
                            \x20   let d = diagnose(r.output);\n\
                            \x20   return d.cause;\n\
                            }\n";

    /// The charge is emitted before the call it pays for, because a call the
    /// budget refuses must not leave a request behind (#32). Printed in
    /// order it landed above the call, indented under the previous one, so
    /// the run appeared to have spent budget on `process.run` - which has
    /// none.
    #[test]
    fn a_budget_charge_is_printed_with_the_call_that_spent_it() {
        let store = temp_store("budget-line");
        let src = write_temp("budget-line.sic", BUDGETED);
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 3, "stderr: {stderr}");
        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        let id = stdout.split_whitespace().next().unwrap().to_string();

        let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &["explain", &id]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(
            stdout.contains("call llm.invoke  (budget: 1 left)"),
            "{stdout}"
        );
        // And nowhere else: `process.run` has no budget, and a line of its own
        // between the two calls is what this used to print.
        assert!(!stdout.contains("\n  budget:"), "{stdout}");
        assert_eq!(stdout.matches("budget:").count(), 1, "{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// Nothing in the VM emits a charge whose call never arrives - the budget
    /// refuses before it charges - so a leftover is a journal cut between the
    /// two. Dropping it would be this reader deciding a run spent nothing
    /// because it could not see what it spent.
    #[test]
    fn a_charge_whose_call_is_missing_is_still_said() {
        let store = temp_store("budget-cut");
        let src = write_temp("budget-cut.sic", BUDGETED);
        let (_, _, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 3);
        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        let id = stdout.split_whitespace().next().unwrap().to_string();

        // Cut the journal directly after the charge, leaving whole lines: the
        // run's own record now ends between a charge and the call it pays for.
        let dir = std::fs::read_dir(&store).unwrap().flatten().next().unwrap();
        let path = dir.path().join("journal.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = text
            .lines()
            .take_while(|l| !l.contains("\"capability_requested\",\"cap\":\"llm.invoke\""))
            .collect();
        assert!(
            kept.iter().any(|l| l.contains("budget_consumed")),
            "the cut has to keep the charge: {text}"
        );
        std::fs::write(&path, format!("{}\n", kept.join("\n"))).unwrap();

        let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["explain", &id]);
        assert_eq!(code, 0);
        assert!(
            stdout.contains("budget: 1 left, for a call this journal does not have"),
            "{stdout}"
        );

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// A waiting run's recording stops where the run stopped, so running out of
    /// answers there is the recording ending rather than the program diverging.
    /// This is the case a comparison that only counted calls would get wrong.
    #[test]
    fn a_waiting_recording_ending_early_is_not_a_difference() {
        let store = temp_store("recheck-waiting");
        let src = write_temp(
            "recheck-waiting.sic",
            "allow {\n\
             \x20   human.approve \"a test\";\n\
             \x20   process.exec \"/usr/bin/true\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let ok = human.approve(\"go?\");\n\
             \x20   if ok {\n\
             \x20       return process.exec(\"/usr/bin/true\");\n\
             \x20   }\n\
             \x20   return 1;\n\
             }\n",
        );
        let id = recorded(&store, &src);

        let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        assert!(stdout.contains("waiting"), "{stdout}");
        assert_eq!(code, 0);

        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["recheck", &id, src.to_str().unwrap()],
        );
        assert_eq!(code, 0, "stderr: {stderr}\n{stdout}");
        assert!(stdout.contains("1 of 1 calls matched"), "{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    /// A model call is bounded by three numbers and gets all three whether or
    /// not the program set them: the driver has a fallback for the deadline
    /// and no limit at all for the tools. A reader told neither assumes the
    /// line is the whole of the bound.
    #[test]
    fn a_plan_says_when_a_model_call_is_bounded_by_nothing() {
        let unbounded = write_temp(
            "deadline-none.sic",
            "type D { cause: String }\n\
             allow { llm.invoke \"m\"; }\n\
             agent diagnose { input: String, output: D }\n\
             fn main() -> LLM<String> {\n\
             \x20   let d = diagnose(\"why\");\n\
             \x20   return d.cause;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", unbounded.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("any number of tool uses"), "{stdout}");
        assert!(stdout.contains("no deadline of its own"), "{stdout}");

        let bounded = write_temp(
            "deadline-set.sic",
            "type D { cause: String }\n\
             allow { llm.invoke \"m\"; }\n\
             agent diagnose {\n\
             \x20   input: String,\n\
             \x20   output: D,\n\
             \x20   tools: 20,\n\
             \x20   deadline: 300000,\n\
             }\n\
             fn main() -> LLM<String> {\n\
             \x20   let d = diagnose(\"why\");\n\
             \x20   return d.cause;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", bounded.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("at most 20 tool use(s)"), "{stdout}");
        assert!(stdout.contains("300000ms per answer"), "{stdout}");
        assert!(!stdout.contains("no deadline"), "{stdout}");

        std::fs::remove_file(unbounded).ok();
        std::fs::remove_file(bounded).ok();
    }

    /// `timeout` on an agent call is refused, and rightly: `deadline` bounds an
    /// answer where `timeout` bounds a call, and keeping them apart is what
    /// stops a program from appearing to have set the other. What was wrong was
    /// the note, which told a reader there was nothing to wait for.
    #[test]
    fn a_timeout_on_an_agent_call_says_where_the_number_goes() {
        let src = write_temp(
            "deadline-timeout.sic",
            "type D { cause: String }\n\
             allow { llm.invoke \"m\"; }\n\
             agent diagnose { input: String, output: D }\n\
             fn main() -> LLM<String> {\n\
             \x20   let d = diagnose(\"why\") timeout 60000;\n\
             \x20   return d.cause;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0330"), "{stderr}");
        assert!(stderr.contains("this is an agent call"), "{stderr}");
        assert!(stderr.contains("`deadline` for wall clock"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A check that reached a live agent would be answering a different
    /// question, so the flag is refused rather than ignored.
    #[test]
    fn recheck_takes_no_driver() {
        let (_, stderr, code) = sic(&["recheck", "abcd1234", "x.sic", "--llm", "tmux:claude"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("takes no driver"), "{stderr}");
    }

    #[test]
    fn recheck_needs_a_run_and_a_file() {
        let (_, stderr, code) = sic(&["recheck", "abcd1234"]);
        assert_eq!(code, 2, "{stderr}");
        assert!(
            stderr.contains("takes a run id and a source file"),
            "{stderr}"
        );
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
        let (_, _, code) =
            sic_with_store(repo_root(), Some(&store), &["run", src.to_str().unwrap()]);
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
        // `workflows/` too: `workflows/ci.sic` is the evidence
        // `docs/design/self-hosting.md` argues from, and a document arguing
        // from a program that no longer compiles is arguing from nothing.
        let mut checked = 0;
        for dir in ["examples", "workflows"] {
            let dir = repo_root().join(dir);
            for entry in std::fs::read_dir(&dir)
                .expect("the directory should exist")
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
        }
        assert!(checked >= 9, "only {checked} programs were checked");
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

    /// A run killed mid-write loses its last line, and that line is the one that
    /// says how the run ended. Losing it is allowed; losing it in silence is not.
    ///
    /// The `run_suspended` case is the one that decides this: the run drops out of
    /// `sic runs --waiting`, which is the list a person or an agent answering runs
    /// works from, and nothing tells them a line could not be read.
    #[test]
    fn a_journal_cut_mid_write_says_so_rather_than_going_quiet() {
        let store = temp_dir("cut-journal");
        let checkpoint = store.join("cut.sicc");
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &[
                "run",
                &example("approval.sic"),
                "--record",
                "--checkpoint",
                checkpoint.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 3, "{stderr}");

        let dir = std::fs::read_dir(&store)
            .expect("the store")
            .find_map(|e| {
                let path = e.ok()?.path();
                path.join("journal.jsonl").exists().then_some(path)
            })
            .expect("one recorded run");
        let journal = dir.join("journal.jsonl");
        let text = std::fs::read_to_string(&journal).expect("readable");

        // Before: it is waiting, and it says so.
        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
        assert!(stdout.contains("waiting"), "{stdout}");

        // Cut the last line in half, the way a killed process does.
        let keep = text.trim_end().rfind('\n').expect("more than one line");
        let cut = &text[..keep + 1 + (text.len() - keep) / 2];
        std::fs::write(&journal, cut).expect("writable");

        for command in [
            vec!["runs", "--waiting"],
            vec!["runs"],
            vec!["explain", dir.file_name().unwrap().to_str().unwrap()],
        ] {
            let (_, stderr, _) = sic_with_store(repo_root(), Some(&store), &command);
            assert!(
                stderr.contains("skipped line"),
                "{command:?} went quiet: {stderr}"
            );
        }

        std::fs::remove_dir_all(&store).ok();
    }
}

/// modules.
mod modules {
    use super::*;

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

    /// A program built from two files is one module, and its node ids used to
    /// restart at zero in each file. The checker keys `res_of` and `type_of` by
    /// them, so the second file's entries overwrote the first's - and what came out
    /// was not a diagnostic.
    ///
    /// The shape matters: a collision does damage only when the two nodes sharing
    /// an id resolved to different things, which is why the imported module's
    /// capability call has to line up with a name in `main`. `examples/import.sic`
    /// went on compiling correctly throughout, which is how this survived.
    #[test]
    fn a_program_in_two_files_keeps_the_capability_call_its_source_has() {
        let main = write_temp_program(
            "two-file-ids",
            &[
                (
                    "main.sic",
                    "import \"lib/reader.sic\";\n\
                 \n\
                 allow {\n\
                 \x20   fs.read \"./secret.txt\";\n\
                 }\n\
                 \n\
                 fn main() -> String {\n\
                 \x20   let a = 0;\n\
                 \x20   let b = a;\n\
                 \x20   let c = b;\n\
                 \x20   let d = c;\n\
                 \x20   return contents(\"./secret.txt\");\n\
                 }\n",
                ),
                (
                    "lib/reader.sic",
                    "requires {\n\
                 \x20   fs.read;\n\
                 }\n\
                 \n\
                 fn contents(path: String) -> String {\n\
                 \x20   return fs.read(path);\n\
                 }\n",
                ),
            ],
        );
        let path = main.to_str().unwrap();

        // It used to reach `unreachable!` in the lowering: a call that resolved to
        // neither a function nor a capability, because the other file had claimed
        // the id.
        let (stdout, stderr, code) = sic(&["plan", path]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("READ"), "{stdout}");
        assert!(stdout.contains("fs.read"), "{stdout}");
        assert!(!stdout.contains("no external effects"), "{stdout}");
        assert!(!stdout.contains("never called"), "{stdout}");

        // And it runs, reading the file the plan said it would.
        let dir = main.parent().unwrap().to_path_buf();
        std::fs::write(dir.join("secret.txt"), "kept").unwrap();
        let (stdout, stderr, code) = sic_in(dir.clone(), &["run", path]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "\"kept\"\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An import may not reach outside the program's directory, and a symbolic link
    /// is the way to do that without writing anything a reader could see.
    ///
    /// Stricter than a capability grant, which follows links, and the difference is
    /// deliberate: a grant names a path on somebody's machine, where `/bin` is a
    /// link on half of them; an import names a file in the program, where a link is
    /// a choice somebody made about this program.
    // A filesystem with links, which is a unix.
    #[cfg(unix)]
    #[test]
    fn an_import_may_not_reach_outside_through_a_symbolic_link() {
        let main = write_temp_program(
            "import-link",
            &[
                (
                    "main.sic",
                    "import \"lib/helper.sic\";\n\nfn main() -> Int {\n    return helper();\n}\n",
                ),
                ("lib/real.sic", "fn helper() -> Int {\n    return 1;\n}\n"),
            ],
        );
        let dir = main.parent().unwrap().to_path_buf();
        let outside = dir.join("outside.sic");
        std::fs::write(&outside, "fn helper() -> Int {\n    return 2;\n}\n").unwrap();

        let link = dir.join("lib").join("helper.sic");
        if std::os::unix::fs::symlink(&outside, &link).is_err() {
            std::fs::remove_dir_all(&dir).ok();
            return;
        }

        let (_, stderr, code) = sic(&["compile", main.to_str().unwrap(), "-o", "/dev/null"]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("symbolic link"), "{stderr}");
        assert!(stderr.contains("outside the program"), "{stderr}");

        // The same program importing the real file compiles, so what is refused is
        // the link and not the shape of the program.
        std::fs::remove_file(&link).unwrap();
        std::fs::copy(dir.join("lib").join("real.sic"), &link).unwrap();
        let (_, stderr, code) = sic(&["compile", main.to_str().unwrap(), "-o", "/dev/null"]);
        assert_eq!(code, 0, "{stderr}");

        std::fs::remove_dir_all(&dir).ok();
    }
}

/// upgrade.
///
/// Every test here stays offline. `sic upgrade` with no `--to` fetches, and a
/// test that reaches GitHub would be testing GitHub. What the fetch computes -
/// which line of SHA256SUMS applies, which release a machine wants - is tested
/// as a unit in `cmd::upgrade`.
mod upgrade {
    use super::*;

    /// Copies the built binary into a directory of its own, so that a test which
    /// replaces a running binary replaces that copy and not the one cargo built.
    ///
    /// These four serve the tests below that replace a running binary, and those
    /// are Linux-only for the reason written on them: the candidate is the
    /// binary with a byte appended, which an ELF loader ignores and a signed
    /// Mach-O does not.
    #[cfg(target_os = "linux")]
    fn install_copy(name: &str, rel: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("sic-test-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_sic"), &path).unwrap();
        path
    }

    #[cfg(target_os = "linux")]
    fn digest_of(path: &std::path::Path) -> String {
        sic_core::Digest::of(&std::fs::read(path).unwrap()).hex()
    }

    /// Runs a binary by its own path, which is what `sic upgrade` needs: the file it
    /// replaces is the one it is running from.
    #[cfg(target_os = "linux")]
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
    fn an_upgrade_without_a_digest_is_refused() {
        let (_, stderr, code) = sic(&["upgrade", "--to", env!("CARGO_BIN_EXE_sic")]);
        assert_eq!(code, 2);
        assert!(stderr.contains("--sha256"), "{stderr}");
    }

    /// A digest with nothing to apply it to would be a promise about a file that
    /// has not been named.
    #[test]
    fn a_digest_without_a_file_is_refused() {
        let (_, stderr, code) = sic(&["upgrade", "--sha256", &"0".repeat(64)]);
        assert_eq!(code, 2);
        assert!(stderr.contains("belongs with `--to`"), "{stderr}");
    }

    #[test]
    fn a_digest_that_does_not_match_refuses_the_upgrade() {
        let zeros = "0".repeat(64);
        let (_, stderr, code) = sic(&[
            "upgrade",
            "--to",
            env!("CARGO_BIN_EXE_sic"),
            "--sha256",
            &zeros,
        ]);
        assert_eq!(code, 1);
        assert!(
            stderr.contains("but the digest it should have is"),
            "{stderr}"
        );
    }

    /// Linux only: the candidate is the binary with a byte appended, which an ELF
    /// loader ignores but a signed Mach-O does not.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_upgrade_replaces_the_running_binary() {
        let installed = install_copy("upgrade-swap", "sic");
        let candidate = installed.with_file_name("candidate");
        let mut bytes = std::fs::read(&installed).unwrap();
        bytes.push(b'\n');
        std::fs::write(&candidate, &bytes).unwrap();
        let digest = digest_of(&candidate);

        let (stdout, stderr, code) = sic_at(
            &installed,
            &[
                "upgrade",
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
        let installed = install_copy("upgrade-check", "sic");
        let candidate = installed.with_file_name("candidate");
        let mut bytes = std::fs::read(&installed).unwrap();
        bytes.push(b'\n');
        std::fs::write(&candidate, &bytes).unwrap();
        let before = digest_of(&installed);
        let digest = digest_of(&candidate);

        let (stdout, stderr, code) = sic_at(
            &installed,
            &[
                "upgrade",
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
        let installed = install_copy("upgrade-not-sic", "sic");
        let candidate = installed.with_file_name("impostor");
        std::fs::write(&candidate, "#!/bin/sh\necho not sic at all\n").unwrap();
        let digest = digest_of(&candidate);
        let before = digest_of(&installed);

        let (_, stderr, code) = sic_at(
            &installed,
            &[
                "upgrade",
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
        let installed = install_copy("upgrade-cargo", ".cargo/bin/sic");
        let candidate = installed.with_file_name("candidate");
        let mut bytes = std::fs::read(&installed).unwrap();
        bytes.push(b'\n');
        std::fs::write(&candidate, &bytes).unwrap();
        let digest = digest_of(&candidate);
        let before = digest_of(&installed);

        let (_, stderr, code) = sic_at(
            &installed,
            &[
                "upgrade",
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

    #[cfg(target_os = "linux")]
    fn leftovers(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }
}

/// arguments.
mod arguments {
    use super::*;

    #[test]
    fn a_program_can_pass_arguments() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/arguments.sic");
        let (stdout, stderr, code) = sic(&["run", path]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("sic: arguments arrived"), "{stdout}");
    }

    #[test]
    fn a_plan_shows_the_arguments_a_grant_pins() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/arguments.sic");
        let (stdout, stderr, code) = sic(&["plan", path]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains(r#"args ["sic:"]"#), "{stdout}");
    }

    /// The prefix is the whole point: a grant on `tmux` that cannot say which pane
    /// is a grant to drive every pane on the machine.
    #[test]
    fn arguments_outside_the_pinned_prefix_are_refused() {
        let entry = write_temp_program(
            "args-prefix",
            &[(
                "main.sic",
                "allow {\n    process.exec \"/bin/echo\" args [\"sic:\"];\n}\n\
             fn main() -> Int {\n    return process.exec(\"/bin/echo\", [\"elsewhere\"]);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("arguments starting"), "{stderr}");
    }

    /// A grant written before arguments existed keeps the authority it had: none
    /// beyond running the file.
    #[test]
    fn a_grant_without_args_allows_none() {
        let entry = write_temp_program(
            "args-empty",
            &[(
                "main.sic",
                "allow {\n    process.exec \"/bin/echo\";\n}\n\
             fn main() -> Int {\n    return process.exec(\"/bin/echo\", [\"anything\"]);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("no arguments"), "{stderr}");
    }

    /// Leaving the vector off is passing an empty one, so every program written
    /// before this change still says what it said.
    #[test]
    fn the_argument_vector_may_be_left_off() {
        let entry = write_temp_program(
            "args-omitted",
            &[(
                "main.sic",
                "allow {\n    process.exec \"/usr/bin/true\";\n}\n\
             fn main() -> Int {\n    return process.exec(\"/usr/bin/true\");\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
    }

    #[test]
    fn only_a_capability_that_takes_arguments_can_pin_them() {
        let entry = write_temp_program(
            "args-wrong-cap",
            &[(
                "main.sic",
                "allow {\n    fs.read \"./x\" args [\"a\"];\n}\nfn main() -> Int {\n    return 1;\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0328"), "{stderr}");
    }

    #[test]
    fn args_needs_a_list_of_strings() {
        let entry = write_temp_program(
            "args-malformed",
            &[(
                "main.sic",
                "allow {\n    process.exec \"/bin/echo\" args [3];\n}\nfn main() -> Int {\n    return 1;\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0213"), "{stderr}");
    }
}

/// reading what a program said.
mod reading_what_a_program_said {
    use super::*;

    /// The case `process.exec` and `process.capture` between them could not
    /// reach: a program that fails *and* prints why.
    #[test]
    fn a_failing_program_gives_up_both_its_code_and_its_output() {
        let src = write_temp(
            "run-both.sic",
            "allow {\n\
             \x20   process.run \"/bin/sh\" args [\"-c\"];\n\
             }\n\
             \n\
             fn main() -> Observed<String> {\n\
             \x20   let r = process.run(\"/bin/sh\", [\"-c\", \"echo two findings; exit 3\"]);\n\
             \x20   if r.code == 3 {\n\
             \x20       return r.output;\n\
             \x20   }\n\
             \x20   // Only reached if the code did not arrive, so the assertion\n\
             \x20   // on the output below is an assertion about both.\n\
             \x20   return process.run(\"/bin/sh\", [\"-c\", \"echo the code was not 3\"]).output;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("two findings"), "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// `process.capture` is unchanged: a program that failed did not produce an
    /// answer worth reading, and the run stops.
    #[test]
    fn capture_still_fails_the_run_where_run_does_not() {
        let src = write_temp(
            "run-capture-still.sic",
            "allow {\n\
             \x20   process.capture \"/bin/sh\" args [\"-c\"];\n\
             }\n\
             \n\
             fn main() -> Observed<String> {\n\
             \x20   return process.capture(\"/bin/sh\", [\"-c\", \"echo said; exit 3\"]);\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("exited 3"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// Reading what a program said is more authority than running it, so the
    /// three are three grants and none covers another.
    #[test]
    fn a_grant_to_capture_is_not_a_grant_to_run() {
        let src = write_temp(
            "run-other-grant.sic",
            "allow {\n\
             \x20   process.capture \"/bin/echo\" args [];\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let r = process.run(\"/bin/echo\", []);\n\
             \x20   return r.code;\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("process.run"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The provenance is on the field that has one, and the rule it enforces is
    /// the same rule `process.capture`'s answer is under.
    #[test]
    fn what_a_run_printed_still_cannot_decide_what_runs() {
        let src = write_temp(
            "run-injection.sic",
            "allow {\n\
             \x20   process.run \"/bin/echo\" args [];\n\
             \x20   process.exec \"/bin/sh\" args [];\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let r = process.run(\"/bin/echo\", []);\n\
             \x20   return process.exec(r.output, none);\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0372"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// The code is an `Int` and not an `Observed<Int>`. Wrapping the record
    /// would make `if r.code == 0` fail to compile, which is the whole reason
    /// the type exists - so this is the assertion that keeps it unwrapped.
    #[test]
    fn an_exit_code_is_an_operand() {
        let src = write_temp(
            "run-code-int.sic",
            "allow {\n\
             \x20   process.run \"/bin/false\" args [];\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let r = process.run(\"/bin/false\", []);\n\
             \x20   return r.code + 1;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert_eq!(stdout.trim(), "2", "{stdout}");
        std::fs::remove_file(src).ok();
    }

    /// A module may not redefine a type the language declares, and `Exit` is
    /// one - the same diagnostic that refuses redefining `Int`.
    #[test]
    fn a_module_may_not_redefine_exit() {
        let src = write_temp(
            "run-redefine.sic",
            "type Exit { code: Int }\n\
             \n\
             fn main() -> Int { return 1; }\n",
        );
        let (_, stderr, code) = sic(&["run", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0345"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// An `Exit` is a record in the arena, and a run that holds one across a
    /// suspension has to find it there afterwards. This is also the round trip
    /// through `responses.jsonl`, which had no object shape before.
    #[test]
    fn an_exit_survives_a_checkpoint_and_replays() {
        let store = temp_store("run-checkpoint");
        let src = write_temp(
            "run-held.sic",
            "allow {\n\
             \x20   process.run \"/bin/sh\" args [\"-c\"];\n\
             \x20   human.approve \"the findings\";\n\
             }\n\
             \n\
             fn main() -> Int {\n\
             \x20   let r = process.run(\"/bin/sh\", [\"-c\", \"echo findings; exit 2\"]);\n\
             \x20   let ok = human.approve(\"ship anyway?\");\n\
             \x20   if ok {\n\
             \x20       return r.code;\n\
             \x20   }\n\
             \x20   return 0;\n\
             }\n",
        );
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", src.to_str().unwrap(), "--record"],
        );
        assert_eq!(code, 3, "stderr: {stderr}");

        let (stdout, _, _) = sic_with_store(repo_root(), Some(&store), &["runs"]);
        let id = stdout.split_whitespace().next().unwrap().to_string();

        let (stdout, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["attach", &id, "--value", "true"],
        );
        assert_eq!(code, 0, "stderr: {stderr}");
        assert_eq!(
            stdout.trim(),
            "2",
            "the code the program held across the wait"
        );

        // And the recorded answer is readable back, which is what replay needs.
        let (stdout, stderr, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
        assert_eq!(code, 0, "stderr: {stderr}\n{stdout}");

        std::fs::remove_file(src).ok();
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn a_program_can_read_what_it_ran() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/capture.sic");
        let (stdout, stderr, code) = sic(&["run", path]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("sic: read back"), "{stdout}");
    }

    /// A plan is read by somebody deciding whether to run this, and "reads what it
    /// says" is a different thing to know than "runs it".
    #[test]
    fn a_plan_distinguishes_reading_from_running() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/capture.sic");
        let (stdout, stderr, code) = sic(&["plan", path]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("CAPTURE"), "{stdout}");
    }

    /// The oldest injection there is: a string a program printed deciding what the
    /// next program runs.
    #[test]
    fn what_a_program_printed_cannot_decide_what_runs() {
        let entry = write_temp_program(
            "capture-injection",
            &[(
                "main.sic",
                "allow {\n    process.capture \"/bin/echo\" args [\"a\"];\n    process.exec \"/bin/echo\";\n}\n\
             fn main() -> Int {\n    let said = process.capture(\"/bin/echo\", [\"a\"]);\n\
             \x20   return process.exec(\"/bin/echo\", [said]);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0372"), "{stderr}");
        assert!(stderr.contains("a program printed it"), "{stderr}");
    }

    /// `approve` is the way through, and it puts a person on the record.
    #[test]
    fn an_approval_lets_an_observed_value_through() {
        let entry = write_temp_program(
            "capture-approved",
            &[(
                "main.sic",
                "allow {\n    process.capture \"/bin/echo\" args [\"a\"];\n\
             \x20   process.exec \"/bin/echo\";\n    human.approve \"running what was read\";\n}\n\
             fn main() -> Int {\n    let said = process.capture(\"/bin/echo\", [\"a\"]);\n\
             \x20   let checked = approve(\"run this?\", said);\n\
             \x20   return process.exec(\"/bin/echo\", [checked]);\n}\n",
            )],
        );
        // It compiles, and stops to ask - which is exit code 3, not a failure.
        let checkpoint = entry.with_extension("sicc");
        let (_, stderr, code) = sic(&[
            "run",
            entry.to_str().unwrap(),
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        assert_eq!(code, 3, "stderr: {stderr}");
        assert!(stderr.contains("running what was read"), "{stderr}");
        std::fs::remove_file(checkpoint).ok();
    }

    #[test]
    fn a_program_that_fails_produces_no_answer() {
        let entry = write_temp_program(
            "capture-failure",
            &[(
                "main.sic",
                "allow {\n    process.capture \"/bin/sh\" args [\"-c\"];\n}\n\
             fn main() -> Observed<String> {\n\
             \x20   return process.capture(\"/bin/sh\", [\"-c\", \"echo trouble >&2; exit 3\"]);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("exited 3"), "{stderr}");
        // stderr is not a value the program receives, but a failure without it
        // says nothing.
        assert!(stderr.contains("trouble"), "{stderr}");
    }

    /// Running something and reading what it says are different authorities, so
    /// one grant does not cover the other.
    #[test]
    fn a_grant_to_run_is_not_a_grant_to_read() {
        let entry = write_temp_program(
            "capture-ungranted",
            &[(
                "main.sic",
                "allow {\n    process.exec \"/bin/echo\";\n}\n\
             fn main() -> Observed<String> {\n    return process.capture(\"/bin/echo\");\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0320"), "{stderr}");
    }
}

/// decisions.
mod decisions {
    use super::*;

    #[test]
    fn a_decision_stops_the_run_and_lists_the_alternatives() {
        let src = example("decision.sic");
        let checkpoint = write_temp("decision.sicc", "");
        let (_, stderr, code) = sic(&["run", &src, "--checkpoint", checkpoint.to_str().unwrap()]);
        assert_eq!(code, 3, "stderr: {stderr}");
        // Numbered from zero: the number a person reads is the one they answer
        // with.
        assert!(stderr.contains("0. the importing program"), "{stderr}");
        assert!(stderr.contains("2. a library declares"), "{stderr}");

        let (stdout, stderr, code) =
            sic(&["resume", checkpoint.to_str().unwrap(), &src, "--value", "2"]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("a library declares"), "{stdout}");
        std::fs::remove_file(checkpoint).ok();
    }

    /// The answer is an index into the program's own list, so nobody answering can
    /// hand back a value that was never offered. The worst an answer can do is
    /// fail the run.
    #[test]
    fn an_answer_outside_the_alternatives_fails_the_run() {
        let src = example("decision.sic");
        let checkpoint = write_temp("decision-bad.sicc", "");
        let (_, _, code) = sic(&["run", &src, "--checkpoint", checkpoint.to_str().unwrap()]);
        assert_eq!(code, 3);

        let (_, stderr, code) =
            sic(&["resume", checkpoint.to_str().unwrap(), &src, "--value", "9"]);
        assert_eq!(code, 1);
        assert!(stderr.contains("outside the list"), "{stderr}");
        std::fs::remove_file(checkpoint).ok();
    }

    /// How many decisions a run will ask of you is the thing a plan is read for.
    #[test]
    fn a_plan_says_how_many_alternatives_a_decision_offers() {
        let (stdout, stderr, code) = sic(&["plan", &example("decision.sic")]);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert!(stdout.contains("CHOOSE"), "{stdout}");
        assert!(stdout.contains("3 options"), "{stdout}");
    }

    #[test]
    fn choosing_needs_its_own_grant() {
        let entry = write_temp_program(
            "choose-ungranted",
            &[(
                "main.sic",
                "allow {\n    human.approve \"deploying\";\n}\n\
             fn main() -> HumanChosen<String> {\n    return choose(\"which?\", [\"a\", \"b\"]);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("E0373"), "{stderr}");
    }

    /// A chosen value may reach a capability that runs something: its text was
    /// written by whoever wrote the program, and a person picked it.
    #[test]
    fn a_chosen_value_may_decide_what_runs() {
        let entry = write_temp_program(
            "choose-then-exec",
            &[(
                "main.sic",
                "allow {\n    human.choose \"which binary\";\n    process.exec \"/bin/echo\" args [\"a\"];\n}\n\
             fn main() -> Int {\n    let which = choose(\"which?\", [\"a\", \"b\"]);\n\
             \x20   return process.exec(\"/bin/echo\", [which]);\n}\n",
            )],
        );
        let checkpoint = entry.with_extension("sicc");
        let (_, stderr, code) = sic(&[
            "run",
            entry.to_str().unwrap(),
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        // It compiles - the point of the test - and stops to ask.
        assert_eq!(code, 3, "stderr: {stderr}");
        std::fs::remove_file(checkpoint).ok();
    }

    #[test]
    fn a_decision_with_no_alternatives_is_not_one() {
        let entry = write_temp_program(
            "choose-empty",
            &[(
                "main.sic",
                // An empty literal needs an annotation, so the list is built as
                // one - which is also the only way this reaches the broker at all.
                "allow {\n    human.choose \"nothing\";\n}\n\
             fn main() -> HumanChosen<String> {\n\
             \x20   return choose(\"which?\", []);\n}\n",
            )],
        );
        let (_, stderr, code) = sic(&["run", entry.to_str().unwrap()]);
        assert_eq!(code, 1);
        assert!(stderr.contains("no alternatives"), "{stderr}");
    }

    /// The reason is the part worth more than the choice, and the question is what
    /// keeps the alternatives that were not taken.
    #[test]
    fn a_reason_is_recorded_next_to_the_answer() {
        let store = write_temp("runs-because", "");
        std::fs::remove_file(&store).ok();
        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &["run", &example("decision.sic"), "--record"],
        );
        assert_eq!(code, 3, "stderr: {stderr}");

        let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["runs", "--waiting"]);
        assert_eq!(code, 0);
        let id = stdout
            .lines()
            .find_map(|l| l.split_whitespace().next().filter(|w| w.len() >= 8))
            .expect("a waiting run")
            .to_string();

        let (_, stderr, code) = sic_with_store(
            repo_root(),
            Some(&store),
            &[
                "attach",
                &id,
                "--value",
                "2",
                "--because",
                "reading a plan still tells you the truth",
            ],
        );
        assert_eq!(code, 0, "stderr: {stderr}");

        let (stdout, _, code) = sic_with_store(repo_root(), Some(&store), &["explain", &id]);
        assert_eq!(code, 0);
        assert!(stdout.contains("asked a person"), "{stdout}");
        // What was not chosen is the whole value of a recorded decision.
        assert!(stdout.contains("1. grants are unioned"), "{stdout}");
        assert!(stdout.contains("answered 2"), "{stdout}");
        assert!(
            stdout.contains("because reading a plan still tells you the truth"),
            "{stdout}"
        );

        // A recorded reason changes nothing about re-running: replay needs the
        // values and nothing else.
        let (_, stderr, code) = sic_with_store(repo_root(), Some(&store), &["replay", &id]);
        assert_eq!(code, 0, "stderr: {stderr}");

        std::fs::remove_dir_all(&store).ok();
    }

    /// A checkpoint is a run's state, not its record, so there is nowhere in it for
    /// a reason to live.
    #[test]
    fn resume_says_it_cannot_record_a_reason() {
        let src = example("decision.sic");
        let checkpoint = write_temp("because-resume.sicc", "");
        let (_, _, code) = sic(&["run", &src, "--checkpoint", checkpoint.to_str().unwrap()]);
        assert_eq!(code, 3);

        let (_, stderr, code) = sic(&[
            "resume",
            checkpoint.to_str().unwrap(),
            &src,
            "--value",
            "2",
            "--because",
            "why not",
        ]);
        assert_eq!(code, 2);
        assert!(stderr.contains("cannot record a reason"), "{stderr}");
        std::fs::remove_file(checkpoint).ok();
    }
}

/// driving an agent CLI: docs/design/driving.md.
mod driving {
    use super::*;

    /// Nothing answers a model call because it happened to be installed, so the
    /// spec has to say what drives what.
    #[test]
    fn a_driver_spec_names_a_multiplexer_and_an_agent() {
        let src = example("driven.sic");

        let (_, stderr, code) = sic(&["run", &src, "--llm", "claude"]);
        assert_eq!(code, 1);
        assert!(stderr.contains("tmux:claude"), "{stderr}");

        let (_, stderr, code) = sic(&["run", &src, "--llm", "screen:claude"]);
        assert_eq!(code, 1);
        assert!(stderr.contains("only one"), "{stderr}");

        // A driver that cannot be opened stops the run before it has done
        // anything, rather than at the first model call.
        let (_, stderr, code) = sic(&["run", &src, "--llm", "tmux:no-such-agent-exists"]);
        assert_eq!(code, 1);
        assert!(stderr.contains("no `no-such-agent-exists`"), "{stderr}");
    }

    /// A replay answers from what was recorded. Reaching a live agent would make it
    /// something else.
    #[test]
    fn replay_takes_no_driver() {
        let (_, stderr, code) = sic(&["replay", "0000", "--llm", "tmux:claude"]);
        assert_eq!(code, 2);
        assert!(stderr.contains("takes no driver"), "{stderr}");
    }

    /// Without a driver a model call defers, which is what it has always done.
    #[test]
    fn without_a_driver_a_model_call_still_waits() {
        let checkpoint = write_temp("driven.sicc", "");
        let (_, stderr, code) = sic(&[
            "run",
            &example("driven.sic"),
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        assert_eq!(code, 3, "{stderr}");
        assert!(stderr.contains("[claude]"), "{stderr}");
        std::fs::remove_file(checkpoint).ok();
    }

    /// The shape an `agent` declares reaches whoever answers - including a person
    /// answering a deferred call, who is told exactly what a model would be told.
    #[test]
    fn a_deferred_model_call_says_what_shape_the_answer_takes() {
        let checkpoint = write_temp("shape.sicc", "");
        let (_, stderr, code) = sic(&[
            "run",
            &example("driven.sic"),
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        assert_eq!(code, 3, "{stderr}");
        assert!(
            stderr.contains("{\"title\": string, \"severity\": integer}"),
            "{stderr}"
        );

        // And the answer of that shape is accepted.
        let (stdout, stderr, code) = sic(&[
            "resume",
            checkpoint.to_str().unwrap(),
            &example("driven.sic"),
            "--value",
            "{\"title\": \"stuck deploy\", \"severity\": 2}",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "\"stuck deploy\"\n");
        std::fs::remove_file(checkpoint).ok();
    }

    /// The plan has to say what the agent may do, and every line has to name where
    /// it is enforced - a gate and a boundary are different things, and a line with
    /// nothing in parentheses would be a claim with no mechanism behind it.
    ///
    /// This is the test for whether any of the authority work worked: if the plan
    /// cannot say it, the manifest did not reach the agent.
    #[test]
    fn a_plan_says_what_the_agent_answering_may_do() {
        let (stdout, stderr, code) = sic(&["plan", &example("driven.sic")]);
        assert_eq!(code, 0, "{stderr}");
        for line in [
            "the agent may not  reach the network        (no tool it has can)",
            "the agent may not  run a shell of its own   (refused by the hook)",
            "the agent may not  use any other tool       (refused by the hook)",
        ] {
            assert!(stdout.contains(line), "{stdout}");
        }
        // The warning this replaced said what the plan did not know. It knows now.
        assert!(
            !stdout.contains("what the agent may do while answering"),
            "{stdout}"
        );

        // And nothing at all on a grant answered by a person: there is no agent.
        let (stdout, _, _) = sic(&["plan", &example("decision.sic")]);
        assert!(!stdout.contains("the agent"), "{stdout}");
    }

    /// The plan said, three lines apart, that the agent may use `/bin/sh`
    /// through the broker and may not run a shell because the hook refuses it.
    /// Both were true about their own mechanism, which is what made it a plan
    /// nobody could approve.
    ///
    /// A `process` grant is the program's until the manifest says `delegable`,
    /// because for that family the constraint does not bound the authority: a
    /// pinned prefix of `["-c"]` scopes nothing, since everything after it is a
    /// command. So the default says so, and the word is what changes it.
    #[test]
    fn a_shell_the_program_may_run_is_not_a_shell_the_agent_may_run() {
        let withheld = write_temp(
            "delegable-off.sic",
            "type D { cause: String }\n\
             allow {\n\
             \x20   process.capture \"/bin/sh\" args [\"-c\"];\n\
             \x20   llm.invoke \"claude-opus-4\";\n\
             }\n\
             agent diagnose { input: String, output: D, budget: 1 }\n\
             fn main() -> LLM<String> {\n\
             \x20   let out = process.capture(\"/bin/sh\", [\"-c\", \"echo x\"]);\n\
             \x20   let d = diagnose(out);\n\
             \x20   return d.cause;\n\
             }\n",
        );
        let (stdout, stderr, code) = sic(&["plan", withheld.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            stdout.contains(
                "the agent may not  use \"/bin/sh\"            (the grant does not say `delegable`)"
            ),
            "{stdout}"
        );
        assert!(
            !stdout.contains("the agent may use"),
            "nothing is offered: {stdout}"
        );

        // With the word, the grant reaches the agent - and the two lines are
        // about two different mechanisms rather than contradicting.
        let shared = write_temp(
            "delegable-on.sic",
            &std::fs::read_to_string(&withheld)
                .unwrap()
                .replace("args [\"-c\"];", "args [\"-c\"] delegable;"),
        );
        let (stdout, stderr, code) = sic(&["plan", shared.to_str().unwrap()]);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            stdout.contains("the agent may use  \"/bin/sh\"                (through the broker)"),
            "{stdout}"
        );
        assert!(
            stdout.contains("delegable"),
            "the grant line says so: {stdout}"
        );

        std::fs::remove_file(withheld).ok();
        std::fs::remove_file(shared).ok();
    }

    /// A path scope bounds what it allows however it is used, so it reaches the
    /// agent without a word. `delegable` on one is a word that would mean
    /// nothing, and accepting it would be worse than refusing it.
    #[test]
    fn only_a_process_capability_takes_delegable() {
        let src = write_temp(
            "delegable-wrong.sic",
            "allow {\n\
             \x20   fs.read \"./examples/greeting.txt\" delegable;\n\
             }\n\
             fn main() -> String {\n\
             \x20   return fs.read(\"./examples/greeting.txt\");\n\
             }\n",
        );
        let (_, stderr, code) = sic(&["plan", src.to_str().unwrap()]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("E0329"), "{stderr}");
        std::fs::remove_file(src).ok();
    }

    /// A call that continues a conversation is not the same act as one that starts
    /// fresh, and a plan that did not say so would describe calls that look
    /// independent and are not.
    #[test]
    fn a_plan_says_when_a_call_keeps_a_conversation() {
        let (stdout, stderr, code) = sic(&["plan", &example("memory.sic")]);
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("in one conversation per task"), "{stdout}");

        let (stdout, _, _) = sic(&["plan", &example("driven.sic")]);
        assert!(!stdout.contains("conversation"), "{stdout}");
    }

    /// A conversation lives in its run's session, and a loose checkpoint does not
    /// say which run it came from. Starting a fresh one and calling it the old one
    /// would change what the run means without saying so.
    #[test]
    fn resume_will_not_pretend_to_continue_a_conversation() {
        let checkpoint = write_temp("memory.sicc", "");
        let (_, _, code) = sic(&[
            "run",
            &example("memory.sic"),
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        assert_eq!(code, 3);

        let (_, stderr, code) = sic(&[
            "resume",
            checkpoint.to_str().unwrap(),
            &example("memory.sic"),
            "--value",
            "{\"file\": \"a.rs\", \"reason\": \"long\"}",
            "--llm",
            "tmux:claude",
        ]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains("keeps a conversation"), "{stderr}");
        assert!(stderr.contains("sic attach"), "{stderr}");

        // Without a driver it resumes as it always did: it answers the first call
        // and stops at the second, with nothing being continued.
        let (_, stderr, code) = sic(&[
            "resume",
            checkpoint.to_str().unwrap(),
            &example("memory.sic"),
            "--value",
            "{\"file\": \"a.rs\", \"reason\": \"long\"}",
            "--checkpoint",
            checkpoint.to_str().unwrap(),
        ]);
        assert_eq!(code, 3, "{stderr}");
        std::fs::remove_file(checkpoint).ok();
    }

    /// The agent's authority is the program's manifest, and a grant its permission
    /// system cannot hold is not dropped and not weakened: it is offered back
    /// through the broker. So a program that grants one still runs.
    ///
    /// `process.exec` is the case. A rule on a shell command would match text that
    /// can invoke anything, and a digest pin has no equivalent at all, so the tool
    /// is denied and the capability arrives at the broker instead.
    #[test]
    fn a_grant_the_agent_cannot_hold_is_routed_rather_than_refused() {
        let src = write_temp(
            "routed.sic",
            concat!(
                "allow {\n",
                "    llm.invoke \"claude\";\n",
                "    process.exec \"/bin/echo\";\n",
                "}\n",
                "\n",
                "fn main() -> Int {\n",
                "    return 7;\n",
                "}\n",
            ),
        );
        let path = src.to_str().unwrap();

        // Far enough to look for the agent, which is past the point where this
        // used to stop. What fails is finding the agent, not enforcing the grant.
        let (_, stderr, code) = sic(&["run", path, "--llm", "tmux:no-such-agent-exists"]);
        assert_eq!(code, 1, "{stderr}");
        assert!(stderr.contains("no `no-such-agent-exists`"), "{stderr}");
        assert!(!stderr.contains("cannot be enforced"), "{stderr}");

        // And without a driver there is no agent at all.
        let (stdout, stderr, code) = sic(&["run", path]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "7\n");
        std::fs::remove_file(src).ok();
    }
}
