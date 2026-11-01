use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn seed_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/source/seed")
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Dir {
    let root = std::env::temp_dir().join(format!("boxt-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Dir(root)
}

fn boxt<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_boxt"))
        .args(args)
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_first_answer_names_scope_verdict_findings_and_not_checked() {
    let out = boxt([
        seed_dir().as_os_str(),
        "--target".as_ref(),
        "docker-default".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("Source only: "), "{text}");
    let verdict = text.find("\ndocker-default  needs Windows  (").unwrap();
    let findings = text.find("\nFindings\n").unwrap();
    let not_checked = text.find("\nNot checked\n").unwrap();
    assert!(verdict < findings && findings < not_checked, "{text}");
    assert!(text.contains("  setuid.c:7  in drop_to(const char *, uid_t, gid_t)\n"));
    assert!(text.contains("Source lines not checked: "));
}

#[test]
fn every_target_gets_a_line_and_runs_are_byte_identical() {
    let first = boxt([seed_dir()]);
    let second = boxt([seed_dir()]);
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert_eq!(first.stdout, second.stdout);
    let text = stdout(&first);
    for target in [
        "docker-default",
        "kubernetes-restricted",
        "openshift-restricted-v2",
        "rootless-podman",
    ] {
        assert!(
            text.contains(&format!("\n{target}  ")),
            "{target} in {text}"
        );
    }
}

#[test]
fn an_unknown_target_fails_before_any_scan() {
    let dir = temp("unknown-target");
    std::fs::write(dir.0.join("a.c"), "int f(void) { return 0; }\n").unwrap();
    let out = boxt([dir.0.as_os_str(), "--target".as_ref(), "docker".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("boxt: unknown target docker"), "{err}");
    assert!(err.contains("docker-default"), "{err}");
}

#[cfg(feature = "fault-fixtures")]
#[test]
fn an_unknown_target_starts_no_worker() {
    use boxt_collector::worker::PID_DIR_ENV;
    let dir = temp("unknown-target-pids");
    let tree = dir.0.join("tree");
    let pids = dir.0.join("pids");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::create_dir_all(&pids).unwrap();
    std::fs::write(
        tree.join("boxt-fixture-spin.c"),
        "int g(void) { return 1; }\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg(&tree)
        .args(["--target", "docker"])
        .env(PID_DIR_ENV, &pids)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(stderr(&out).starts_with("boxt: unknown target docker"));
    assert_eq!(std::fs::read_dir(&pids).unwrap().count(), 0, "a worker ran");
}

#[test]
fn an_input_named_like_a_hidden_subcommand_is_scanned() {
    let dir = temp("named-worker");
    for name in ["worker", "collect"] {
        let input = dir.0.join(name);
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("a.c"), "int f(void) { return 0; }\n").unwrap();
    }
    for name in ["worker", "collect"] {
        let out = Command::new(env!("CARGO_BIN_EXE_boxt"))
            .arg(name)
            .current_dir(&dir.0)
            .env_remove("CLICOLOR_FORCE")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{name}: {}", stderr(&out));
        assert!(stdout(&out).starts_with("Source only: "), "{name}");
    }
}

#[test]
fn help_and_version_exit_0_on_stdout() {
    for flag in ["--help", "--version"] {
        let out = boxt([flag]);
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert!(!out.stdout.is_empty(), "{flag}");
    }
    assert!(stdout(&boxt(["--version"])).starts_with("boxt "));
}

#[test]
fn a_bad_limit_or_jobs_exits_2_with_empty_stdout() {
    for args in [
        vec![
            seed_dir().into_os_string(),
            "--limit".into(),
            "bogus".into(),
        ],
        vec![
            seed_dir().into_os_string(),
            "--limit".into(),
            "bogus=1".into(),
        ],
        vec![seed_dir().into_os_string(), "--jobs".into(), "0".into()],
    ] {
        let out = boxt(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(stderr(&out).starts_with("boxt: "), "{args:?}");
    }
    assert!(
        stderr(&boxt([
            seed_dir().into_os_string(),
            "--jobs".into(),
            "0".into()
        ]))
        .contains("--jobs")
    );
}

#[test]
fn a_small_file_size_limit_lists_the_file_as_not_checked() {
    let dir = temp("file-size");
    std::fs::write(dir.0.join("small.c"), "int f(void);\n").unwrap();
    std::fs::write(
        dir.0.join("big.c"),
        "int g(void) { return 1; }\n".repeat(20),
    )
    .unwrap();
    let out = boxt([
        dir.0.as_os_str(),
        "--limit".as_ref(),
        "file-size=64".as_ref(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    let not_checked = text.find("\nNot checked\n").unwrap();
    let line = text.find("\n  big.c: over the file-size limit (").unwrap();
    assert!(line > not_checked, "{text}");
}

#[test]
fn forced_colour_only_adds_escape_sequences() {
    let plain = boxt([seed_dir()]);
    let coloured = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg(seed_dir())
        .env("CLICOLOR_FORCE", "1")
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert_eq!(coloured.status.code(), Some(0));
    assert!(coloured.stdout.contains(&0x1b));
    assert_eq!(strip_sgr(&coloured.stdout), plain.stdout);
}

fn strip_sgr(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[test]
fn a_bad_flag_prints_usage_and_exits_2() {
    let out = boxt(["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("boxt: "), "{err}");
    assert!(err.contains("Usage"), "{err}");
    let out = boxt::<[&str; 0], &str>([]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn nothing_read_exits_2_with_empty_stdout() {
    let dir = temp("empty");
    let out = boxt([&dir.0]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(
        err.starts_with("boxt: ") && err.contains("nothing") && err.contains("read"),
        "{err}"
    );
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn redirected_output_has_no_escape_byte() {
    let plain = boxt([seed_dir()]);
    assert!(!plain.stdout.contains(&0x1b));
    let no_color = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg(seed_dir())
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .unwrap();
    assert!(!no_color.stdout.contains(&0x1b));
    assert_eq!(no_color.stdout, plain.stdout);
}

const HOSTILE_LITERAL: &str = "\x1b]8;;http://example.invalid\x07x\x1b]8;;\x07\x1b[31m";

#[test]
fn hostile_names_and_literals_print_as_visible_escapes() {
    let dir = temp("hostile");
    let name = if cfg!(windows) {
        "ev\u{202e}il\u{200b}.c"
    } else {
        "ev\u{202e}il\t\u{200b}\x1b[31m.c"
    };
    let body = format!(
        "#include <unistd.h>\n\nint drop_all(uid_t uid)\n{{\n    const char *s = \"{}\";\n    (void)s;\n    return setuid(uid);\n}}\n",
        HOSTILE_LITERAL
            .replace('\x1b', "\\x1b")
            .replace('\x07', "\\x07")
    );
    std::fs::write(dir.0.join(name), body).unwrap();
    let out = boxt([&dir.0]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!out.stdout.contains(&0x1b));
    assert!(!out.stdout.contains(&0x07));
    let text = stdout(&out);
    let shown = if cfg!(windows) {
        "ev\\u{202e}il\\u{200b}.c:7"
    } else {
        "ev\\u{202e}il\\x09\\u{200b}\\x1b[31m.c:7"
    };
    assert!(text.contains(shown), "{text}");
    assert!(!text.contains('\u{202e}') && !text.contains('\u{200b}'));
}

#[cfg(feature = "fault-fixtures")]
#[test]
fn a_main_process_panic_exits_2() {
    let dir = temp("panic");
    let input = dir.0.join("boxt-fixture-main-panic");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(input.join("a.c"), "int f(void) { return 0; }\n").unwrap();
    let out = boxt([&input]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("boxt: internal error: "), "{err}");
}

#[cfg(all(feature = "fault-fixtures", target_os = "linux"))]
#[test]
fn an_interrupt_leaves_no_worker_and_no_output() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use boxt_collector::worker::PID_DIR_ENV;

    for signal in ["TERM", "INT"] {
        let dir = temp(&format!("interrupt-{signal}"));
        let tree = dir.0.join("tree");
        let pids = dir.0.join("pids");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(&pids).unwrap();
        std::fs::write(tree.join("a.c"), "int f(void) { return 0; }\n").unwrap();
        std::fs::write(
            tree.join("boxt-fixture-spin.c"),
            "int g(void) { return 1; }\n",
        )
        .unwrap();
        let parent = Command::new(env!("CARGO_BIN_EXE_boxt"))
            .arg(&tree)
            .env(PID_DIR_ENV, &pids)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let workers = loop {
            let workers: Vec<u32> = std::fs::read_dir(&pids)
                .unwrap()
                .flatten()
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))
                .collect();
            if !workers.is_empty() || Instant::now() > deadline {
                break workers;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!workers.is_empty(), "a worker started");
        let sent = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(parent.id().to_string())
            .status()
            .unwrap();
        assert!(sent.success());
        let out = parent.wait_with_output().unwrap();
        assert!(out.stdout.is_empty(), "SIG{signal} left output");
        let deadline = Instant::now() + Duration::from_secs(10);
        while workers.iter().any(|&w| running(w)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let survivors: Vec<u32> = workers.iter().copied().filter(|&w| running(w)).collect();
        for &w in &survivors {
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(w.to_string())
                .status();
        }
        assert!(survivors.is_empty(), "a worker outlived SIG{signal}");
    }
}

#[cfg(all(feature = "fault-fixtures", target_os = "linux"))]
fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rfind(')').map(|i| stat[i + 2..i + 3].to_string()))
        .is_some_and(|state| state != "Z")
}
