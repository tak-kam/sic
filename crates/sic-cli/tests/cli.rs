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
}

/// trust and provenance.
mod trust {
    use super::*;

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
