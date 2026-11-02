use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use boxt_collector::boxt_core::fact::Fact;
use boxt_collector::boxt_core::limits::Limits;
use boxt_collector::boxt_core::settings::{Encoding, EncodingDecl, PathPattern, Settings};
use boxt_collector::boxt_core::{SortedVec, Text};
use boxt_collector::path::{match_text, text_from_path};
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{RunOptions, run_with};

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ROOT_CALL: &str = "#include <unistd.h>\nint f(void) { return setuid(0); }\n";

fn repo(name: &str) -> Dir {
    let root = std::env::temp_dir().join(format!("boxt-excludes-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".git")).unwrap();
    write(&root.join("src/main.c"), ROOT_CALL.as_bytes());
    Dir(root)
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn boxt<I, S>(dir: &Dir, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let pids = dir.0.join("..").join(format!(
        "{}-pids",
        dir.0.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&pids);
    std::fs::create_dir_all(&pids).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_boxt"));
    command
        .args(args)
        .current_dir(&dir.0)
        .env_remove("CLICOLOR_FORCE");
    #[cfg(feature = "fault-fixtures")]
    command.env(boxt_collector::worker::PID_DIR_ENV, &pids);
    let out = command.output().unwrap();
    let ran = std::fs::read_dir(&pids).unwrap().count();
    let _ = std::fs::remove_dir_all(&pids);
    assert_eq!(ran, 0, "a worker got a job for a spin fixture");
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn ok(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(0), "{}", stderr(out));
    stdout(out)
}

fn refused(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(2), "{}", stdout(out));
    assert!(out.stdout.is_empty(), "{}", stdout(out));
    let err = stderr(out);
    assert!(err.starts_with("boxt: "), "{err}");
    err
}

fn counts(report: &str) -> &str {
    report
        .lines()
        .find(|l| l.contains(" files read · "))
        .unwrap_or_else(|| panic!("no counts line: {report}"))
        .trim()
}

fn has_line(report: &str, line: &str) -> bool {
    report.lines().any(|l| l.trim_end() == line)
}

fn spin_fixture(path: &Path) {
    write(path, b"int g(void) { return 1; }\n");
}

#[test]
fn an_excluded_tree_gets_no_job_and_is_counted_under_its_pattern() {
    let dir = repo("tree");
    write(&dir.0.join("third_party/x.c"), b"int x;\n");
    write(&dir.0.join("third_party/notes.txt"), b"vendored\n");
    spin_fixture(&dir.0.join("third_party/deep/boxt-fixture-spin.c"));
    write(
        &dir.0.join("boxt.yaml"),
        b"schema: \"1.0\"\nexclude: [third_party/**]\n",
    );
    let report = ok(&boxt(&dir, ["."]));
    assert!(
        has_line(&report, "  exclude third_party/** · 3 files"),
        "{report}"
    );
    assert!(!report.contains("third_party/x.c"), "{report}");
    assert!(!report.contains("notes.txt"), "{report}");
    std::fs::remove_dir_all(dir.0.join("third_party")).unwrap();
    let without = ok(&boxt(&dir, ["."]));
    assert_eq!(counts(&report), counts(&without));
    assert!(
        has_line(&without, "  exclude third_party/** · 0 files"),
        "{without}"
    );
}

#[test]
fn a_star_stops_at_a_slash() {
    let dir = repo("star");
    spin_fixture(&dir.0.join("src/boxt-fixture-spin.c"));
    write(&dir.0.join("src/a/b.c"), ROOT_CALL.as_bytes());
    let report = ok(&boxt(&dir, [".", "--exclude", "src/*.c"]));
    assert!(has_line(&report, "  exclude src/*.c · 2 files"), "{report}");
    assert!(report.contains("src/a/b.c:2"), "{report}");
    assert!(!report.contains("src/main.c:2"), "{report}");
    assert!(counts(&report).starts_with("1 files read"), "{report}");
}

#[test]
fn a_file_pattern_is_relative_to_the_project_file_folder() {
    let dir = repo("base");
    write(
        &dir.0.join("sub/boxt.yaml"),
        b"schema: \"1.0\"\nexclude: [gen/**]\n",
    );
    spin_fixture(&dir.0.join("sub/gen/boxt-fixture-spin.c"));
    write(&dir.0.join("gen/x.c"), ROOT_CALL.as_bytes());
    let report = ok(&boxt(&dir, [".", "--project", "sub/boxt.yaml"]));
    assert!(has_line(&report, "  exclude gen/** · 1 file"), "{report}");
    assert!(report.contains("gen/x.c:2"), "{report}");
    assert!(!report.contains("sub/gen/"), "{report}");
    assert!(
        has_line(&report, "  excluded sub/boxt.yaml · boxt's project file"),
        "{report}"
    );
}

#[test]
fn an_exclude_flag_replaces_the_file_list() {
    let dir = repo("replace");
    write(&dir.0.join("a/x.c"), ROOT_CALL.as_bytes());
    spin_fixture(&dir.0.join("b/boxt-fixture-spin.c"));
    write(
        &dir.0.join("boxt.yaml"),
        b"schema: \"1.0\"\nexclude: [a/**]\n",
    );
    let report = ok(&boxt(&dir, [".", "--exclude", "b/**"]));
    assert!(has_line(&report, "  exclude b/** · 1 file"), "{report}");
    assert!(!report.contains("exclude a/**"), "{report}");
    assert!(report.contains("a/x.c:2"), "{report}");
}

#[cfg(windows)]
#[test]
fn on_windows_an_exclude_folds_case_after_an_exact_miss() {
    let dir = repo("fold");
    spin_fixture(&dir.0.join("Third_Party/boxt-fixture-spin.c"));
    let report = ok(&boxt(&dir, [".", "--exclude", "third_party/**"]));
    assert!(
        has_line(&report, "  exclude third_party/** · 1 file"),
        "{report}"
    );
}

#[cfg(unix)]
#[test]
fn off_windows_an_exclude_never_folds_case() {
    let dir = repo("no-fold");
    write(&dir.0.join("Third_Party/a.c"), ROOT_CALL.as_bytes());
    let report = ok(&boxt(&dir, [".", "--exclude", "third_party/**"]));
    assert!(
        has_line(&report, "  exclude third_party/** · 0 files"),
        "{report}"
    );
    assert!(report.contains("Third_Party/a.c:2"), "{report}");
}

#[test]
fn the_project_file_in_the_tree_is_an_own_file_not_a_file_not_checked() {
    let dir = repo("own");
    write(&dir.0.join("boxt.yaml"), b"schema: \"1.0\"\n");
    let report = ok(&boxt(&dir, [".", "--all"]));
    assert!(
        has_line(&report, "  excluded boxt.yaml · boxt's project file"),
        "{report}"
    );
    assert!(
        counts(&report).starts_with("1 files read · 0 not checked"),
        "{report}"
    );
    let named = ok(&boxt(&dir, [".", "--project", "boxt.yaml", "--all"]));
    assert!(
        has_line(&named, "  excluded boxt.yaml · boxt's project file"),
        "{named}"
    );
}

fn collect_with(dir: &Dir, encodings: Vec<EncodingDecl>) -> Vec<Fact> {
    let settings = Settings {
        inputs: vec![text_from_path(&dir.0)],
        targets: SortedVec::new(),
        limits: Limits::default(),
        encodings,
        excludes: Vec::new(),
        own_files: Vec::new(),
        ignored_macros: Default::default(),
        jobs: None,
        compile_database: None,
    };
    let options = RunOptions {
        program: PathBuf::from(env!("CARGO_BIN_EXE_boxt")),
        args: vec![OsString::from(WORKER_SUBCOMMAND)],
        env: Vec::new(),
        measure: false,
    };
    let (collected, _) = run_with(&settings, &options).unwrap();
    collected.facts.into_iter().flat_map(|f| f.facts).collect()
}

#[test]
fn a_declared_windows_1252_file_gives_its_literal_decoded() {
    let dir = repo("cp1252");
    write(
        &dir.0.join("legacy/old.c"),
        b"const char *s = \"caf\xe9\";\n",
    );
    let undeclared = ok(&boxt(&dir, [".", "--all"]));
    assert!(
        has_line(&undeclared, "  legacy/old.c: encoding undeclared"),
        "{undeclared}"
    );
    write(
        &dir.0.join("boxt.yaml"),
        b"schema: \"1.0\"\nencodings: [{path: legacy/**, encoding: windows-1252}]\n",
    );
    let declared = ok(&boxt(&dir, [".", "--all"]));
    assert!(!declared.contains("legacy/old.c"), "{declared}");
    assert!(
        counts(&declared).starts_with("2 files read · 0 not checked"),
        "{declared}"
    );
    let flagged = ok(&boxt(
        &dir,
        [".", "--all", "--encoding", "legacy/**=WINDOWS-1252"],
    ));
    assert_eq!(counts(&flagged), counts(&declared));

    let facts = collect_with(
        &dir,
        vec![EncodingDecl {
            pattern: PathPattern {
                base: match_text(&dir.0).unwrap(),
                pattern: Text::from("legacy/**"),
            },
            encoding: Encoding::WindowsCodePage(1252),
        }],
    );
    assert!(
        facts.iter().any(|f| matches!(f,
            Fact::StringLiteral { value, .. } if value == &Text::from("café"))),
        "{facts:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_linked_file_input_matches_its_encoding_by_the_name_given() {
    let dir = repo("linked-input");
    write(
        &dir.0.join("legacy/old.c"),
        b"const char *s = \"caf\xe9\";\n",
    );
    std::os::unix::fs::symlink("legacy/old.c", dir.0.join("link.c")).unwrap();
    let report = ok(&boxt(
        &dir,
        ["link.c", "--all", "--encoding", "link.c=windows-1252"],
    ));
    assert!(
        counts(&report).starts_with("1 files read · 0 not checked"),
        "{report}"
    );
}

#[test]
fn bytes_invalid_for_the_declared_encoding_are_not_checked() {
    let dir = repo("invalid");
    write(&dir.0.join("legacy/bad.c"), b"int a; /* \xff */\n");
    let report = ok(&boxt(&dir, [".", "--all", "--encoding", "legacy/**=utf-8"]));
    assert!(
        report.contains("legacy/bad.c: invalid for its declared encoding"),
        "{report}"
    );
    assert!(
        counts(&report).starts_with("1 files read · 1 not checked"),
        "{report}"
    );
}

#[test]
fn a_code_page_the_workers_cannot_decode_exits_2_before_any_job() {
    let dir = repo("cp437");
    spin_fixture(&dir.0.join("src/boxt-fixture-spin.c"));
    write(
        &dir.0.join("boxt.yaml"),
        b"schema: \"1.0\"\nencodings: [{path: legacy/**, encoding: windows-437}]\n",
    );
    let err = refused(&boxt(&dir, ["."]));
    assert!(
        err.contains("encodings[0].encoding: 'windows-437'"),
        "{err}"
    );
    std::fs::remove_file(dir.0.join("boxt.yaml")).unwrap();
    let err = refused(&boxt(&dir, [".", "--encoding", "legacy/**=utf-16"]));
    assert!(err.contains("--encoding legacy/**=utf-16"), "{err}");
    assert!(err.contains("utf-16le or utf-16be"), "{err}");
    let err = refused(&boxt(&dir, [".", "--encoding", "legacy/**=cp1252"]));
    assert!(err.contains("'cp1252' is not an encoding"), "{err}");
}

#[test]
fn a_bad_pattern_exits_2_naming_its_key_or_flag() {
    let dir = repo("bad-pattern");
    spin_fixture(&dir.0.join("src/boxt-fixture-spin.c"));
    for (text, needle) in [
        ("exclude: [\"../x/**\"]", "exclude[0]: '../x/**' is refused"),
        (
            "exclude: [\"\"]",
            "exclude[0]: '' is refused: the pattern is empty",
        ),
        ("exclude: [ok/**, /abs]", "exclude[1]: '/abs' is refused"),
    ] {
        write(
            &dir.0.join("boxt.yaml"),
            format!("schema: \"1.0\"\n{text}\n").as_bytes(),
        );
        let err = refused(&boxt(&dir, ["."]));
        assert!(err.contains(needle), "{text}: {err}");
    }
    std::fs::remove_file(dir.0.join("boxt.yaml")).unwrap();
    let err = refused(&boxt(&dir, [".", "--exclude", "../x/**"]));
    assert!(
        err.contains("--exclude ../x/**: the pattern is refused"),
        "{err}"
    );
}

#[test]
fn help_lists_the_exclude_and_encoding_flags() {
    let out = Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg("--help")
        .output()
        .unwrap();
    let help = stdout(&out);
    assert!(help.contains("--exclude <PATTERN>"), "{help}");
    assert!(
        help.contains("Leave out the files a path pattern matches"),
        "{help}"
    );
    assert!(help.contains("--encoding <PATTERN=NAME>"), "{help}");
    assert!(help.contains("legacy/**=windows-1252"), "{help}");
}

#[test]
fn the_same_file_and_flags_give_byte_identical_reports() {
    let dir = repo("identical");
    write(&dir.0.join("third_party/x.c"), b"int x;\n");
    write(
        &dir.0.join("legacy/old.c"),
        b"const char *s = \"caf\xe9\";\n",
    );
    write(
        &dir.0.join("boxt.yaml"),
        b"schema: \"1.0\"\nexclude: [third_party/**, gen/**]\nencodings: [{path: legacy/**, encoding: latin-1}]\n",
    );
    let args = [
        ".",
        "--jobs",
        "3",
        "--exclude",
        "third_party/**",
        "--exclude",
        "x/**",
    ];
    let first = boxt(&dir, args);
    let second = boxt(&dir, args);
    ok(&first);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
}
