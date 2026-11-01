use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Dir {
    let root = std::env::temp_dir().join(format!("boxt-project-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Dir(root)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn repo(name: &str) -> Dir {
    let dir = temp(name);
    std::fs::create_dir_all(dir.0.join("repo/.git")).unwrap();
    write(
        &dir.0.join("repo/src/net/a.c"),
        "#include <unistd.h>\nint f(void) { return setuid(0); }\n",
    );
    std::fs::create_dir_all(dir.0.join("elsewhere")).unwrap();
    dir
}

fn boxt_in<I, S>(cwd: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_boxt"))
        .args(args)
        .current_dir(cwd)
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

fn ok(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(0), "{}", stderr(out));
    stdout(out)
}

fn target_rows(report: &str) -> Vec<String> {
    [
        "docker-default",
        "kubernetes-restricted",
        "openshift-restricted-v2",
        "rootless-podman",
    ]
    .into_iter()
    .filter(|t| report.lines().any(|l| l.starts_with(&format!("  {t}  "))))
    .map(str::to_owned)
    .collect()
}

fn shown(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

#[test]
fn a_file_above_the_first_input_is_found_from_any_working_folder() {
    let dir = repo("found");
    let file = dir.0.join("repo/boxt.yaml");
    write(&file, "schema: \"1.0\"\ntargets: [docker-default]\n");
    let input = dir.0.join("repo/src/net");
    let out = boxt_in(&dir.0.join("elsewhere"), [&input]);
    let report = ok(&out);
    assert!(
        report.contains(&format!("\n  project file: {}\n", shown(&file))),
        "{report}"
    );
    assert_eq!(target_rows(&report), ["docker-default"], "{report}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
}

#[test]
fn a_file_input_searches_from_its_folder_and_shows_the_path_joined_from_the_input() {
    let dir = repo("file-input");
    write(&dir.0.join("repo/src/boxt.yaml"), "schema: \"1.0\"\n");
    let out = boxt_in(&dir.0, ["repo/./src/net/a.c"]);
    let report = ok(&out);
    assert!(
        report.contains(&format!(
            "\n  project file: {}\n",
            shown(&dir.0.join("repo/src/boxt.yaml"))
        )),
        "{report}"
    );
    assert!(!report.contains("lies above"), "{report}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
}

#[test]
fn an_input_with_dot_dot_resolves_by_name() {
    let dir = repo("dot-dot");
    write(&dir.0.join("repo/boxt.yaml"), "schema: \"1.0\"\n");
    let report = ok(&boxt_in(&dir.0, ["elsewhere/../repo/src"]));
    assert!(
        report.contains(&format!(
            "\n  project file: {}\n",
            shown(&dir.0.join("repo/boxt.yaml"))
        )),
        "{report}"
    );
}

#[test]
fn the_nearest_folder_wins() {
    let dir = repo("nearest");
    let inner = dir.0.join("repo/src/boxt.yaml");
    write(&inner, "schema: \"1.0\"\ntargets: [rootless-podman]\n");
    write(
        &dir.0.join("repo/.boxt.yaml"),
        "schema: \"1.0\"\ntargets: [docker-default]\n",
    );
    let out = boxt_in(&dir.0, [dir.0.join("repo/src/net")]);
    let report = ok(&out);
    assert!(
        report.contains(&format!("\n  project file: {}\n", shown(&inner))),
        "{report}"
    );
    assert_eq!(target_rows(&report), ["rootless-podman"], "{report}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
}

#[test]
fn the_hidden_name_is_found_too() {
    let dir = repo("hidden-name");
    let file = dir.0.join("repo/.boxt.yaml");
    write(&file, "schema: \"1.0\"\ntargets: [rootless-podman]\n");
    let report = ok(&boxt_in(&dir.0, [dir.0.join("repo/src")]));
    assert!(report.contains(&format!("project file: {}\n", shown(&file))));
    assert_eq!(target_rows(&report), ["rootless-podman"], "{report}");
}

#[test]
fn a_file_named_by_flag_wins_over_the_one_found() {
    let dir = repo("flag-wins");
    write(
        &dir.0.join("repo/boxt.yaml"),
        "schema: \"1.0\"\ntargets: [docker-default]\n",
    );
    write(
        &dir.0.join("elsewhere/other"),
        "schema: \"1.0\"\ntargets: [rootless-podman]\n",
    );
    let report = ok(&boxt_in(
        &dir.0.join("elsewhere"),
        [
            dir.0.join("repo/src").as_os_str(),
            "--project".as_ref(),
            "other".as_ref(),
        ],
    ));
    assert!(
        report.contains("\n  project file: other (named by --project)\n"),
        "{report}"
    );
    assert_eq!(target_rows(&report), ["rootless-podman"], "{report}");
}

#[test]
fn flags_merge_over_the_file_setting_by_setting() {
    let dir = repo("merge");
    write(
        &dir.0.join("repo/boxt.yaml"),
        "schema: \"1.0\"\ntargets: [docker-default]\nlimits:\n  file-size: 64\n  work: 1000000\n",
    );
    write(
        &dir.0.join("repo/src/big.c"),
        &"int g(void) { return 1; }\n".repeat(20),
    );
    let input = dir.0.join("repo/src");
    let report = ok(&boxt_in(
        &dir.0,
        [
            input.as_os_str(),
            "--target".as_ref(),
            "rootless-podman".as_ref(),
            "--limit".as_ref(),
            "memory=1048576".as_ref(),
        ],
    ));
    assert_eq!(target_rows(&report), ["rootless-podman"], "{report}");
    assert!(
        report.contains("\n  big.c: over the file-size limit ("),
        "{report}"
    );
    let report = ok(&boxt_in(
        &dir.0,
        [
            input.as_os_str(),
            "--limit".as_ref(),
            "file-size=1048576".as_ref(),
        ],
    ));
    assert_eq!(target_rows(&report), ["docker-default"], "{report}");
    assert!(!report.contains("big.c"), "{report}");
}

#[test]
fn with_no_file_up_to_the_root_defaults_apply() {
    let dir = repo("none");
    let report = ok(&boxt_in(&dir.0, [dir.0.join("repo/src")]));
    assert!(
        report.contains("\n  project file: none found\n"),
        "{report}"
    );
    assert_eq!(target_rows(&report).len(), 4, "{report}");
}

#[test]
fn a_file_above_the_vcs_root_is_used_with_a_warning() {
    let dir = repo("above-root");
    let file = dir.0.join("boxt.yaml");
    write(&file, "schema: \"1.0\"\ntargets: [docker-default]\n");
    let out = boxt_in(&dir.0, [dir.0.join("repo/src")]);
    let report = ok(&out);
    assert_eq!(target_rows(&report), ["docker-default"], "{report}");
    assert!(
        report
            .contains("│ The project file lies above the version-control root of the first input;"),
        "{report}"
    );
    assert!(report.contains(&format!("project file: {}\n", shown(&file))));
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!(
            "boxt: warning: the project file {} lies above {}, the version-control root",
            shown(&file),
            shown(&dir.0.join("repo"))
        )),
        "{err}"
    );
}

#[test]
fn an_hg_root_counts_as_the_vcs_root() {
    let dir = repo("hg-root");
    std::fs::remove_dir(dir.0.join("repo/.git")).unwrap();
    std::fs::create_dir(dir.0.join("repo/.hg")).unwrap();
    let file = dir.0.join("boxt.yaml");
    write(&file, "schema: \"1.0\"\n");
    let out = boxt_in(&dir.0, [dir.0.join("repo/src")]);
    let report = ok(&out);
    assert!(report.contains("│ The project file lies above the version-control root"));
    assert!(
        stderr(&out).starts_with(&format!(
            "boxt: warning: the project file {} lies above {},",
            shown(&file),
            shown(&dir.0.join("repo"))
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_file_at_the_vcs_root_or_named_by_flag_gives_no_warning() {
    let dir = repo("at-root");
    write(&dir.0.join("repo/boxt.yaml"), "schema: \"1.0\"\n");
    write(&dir.0.join("named.yaml"), "schema: \"1.0\"\n");
    let out = boxt_in(&dir.0, [dir.0.join("repo/src")]);
    assert!(!ok(&out).contains("lies above"));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    let out = boxt_in(
        &dir.0.join("repo"),
        [
            "src".as_ref(),
            "--project".as_ref(),
            dir.0.join("named.yaml").as_os_str(),
        ],
    );
    assert!(!ok(&out).contains("lies above"));
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
}

#[test]
fn two_runs_with_one_file_and_the_same_flags_are_byte_identical() {
    let dir = repo("identical");
    write(
        &dir.0.join("boxt.yaml"),
        "schema: \"1.0\"\njobs: 2\nlimits: {work: 1000000}\n",
    );
    let args = [
        dir.0.join("repo/src").into_os_string(),
        "--target".into(),
        "docker-default".into(),
    ];
    let first = boxt_in(&dir.0, &args);
    let second = boxt_in(&dir.0, &args);
    ok(&first);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
}

#[test]
fn hidden_subcommands_read_no_project_file() {
    let dir = repo("hidden-subcommand");
    write(&dir.0.join("repo/boxt.yaml"), "colour: red\n");
    let out = boxt_in(
        &dir.0,
        ["__collect".as_ref(), dir.0.join("repo/src").as_os_str()],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("outcomes "), "{}", stdout(&out));
}

fn refused(dir: &Dir, args: &[&OsStr]) -> String {
    write(
        &dir.0.join("repo/src/boxt-fixture-spin.c"),
        "int g(void) { return 1; }\n",
    );
    let pids = dir.0.join("pids");
    std::fs::create_dir_all(&pids).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_boxt"));
    command
        .arg(dir.0.join("repo/src"))
        .args(args)
        .current_dir(dir.0.join("elsewhere"))
        .env_remove("CLICOLOR_FORCE");
    #[cfg(feature = "fault-fixtures")]
    command.env(boxt_collector::worker::PID_DIR_ENV, &pids);
    let out = command.output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    assert_eq!(std::fs::read_dir(&pids).unwrap().count(), 0, "a worker ran");
    stderr(&out)
}

fn refused_file(name: &str, text: impl AsRef<[u8]>) -> (String, String) {
    let dir = repo(name);
    let file = dir.0.join("repo/boxt.yaml");
    std::fs::write(&file, text).unwrap();
    (refused(&dir, &[]), shown(&file))
}

#[test]
fn an_unknown_key_is_named_before_any_worker_starts() {
    for (text, key) in [
        ("schema: \"1.0\"\ncolour: red\n", "'colour'"),
        ("schema: \"1.0\"\nlimits: {wrk: 5}\n", "'limits.wrk'"),
        ("schema: \"1.0\"\ninputs: [src]\n", "'inputs'"),
    ] {
        let (err, file) = refused_file("unknown-key", text);
        assert!(
            err.starts_with(&format!("boxt: {file}: unknown key {key}")),
            "{text}: {err}"
        );
    }
}

#[test]
fn a_bad_value_is_named_before_any_worker_starts() {
    for (text, key) in [
        ("schema: \"1.0\"\njobs: many\n", "jobs: "),
        ("schema: \"1.0\"\nlimits: {work: 0}\n", "limits.work: "),
        ("schema: \"1.0\"\ntargets: docker-default\n", "targets: "),
    ] {
        let (err, file) = refused_file("bad-value", text);
        assert!(
            err.starts_with(&format!("boxt: {file}: {key}")),
            "{text}: {err}"
        );
    }
}

#[test]
fn a_newer_version_names_both_versions() {
    let (err, file) = refused_file("newer", "schema: \"1.9\"\n");
    assert!(err.starts_with(&format!("boxt: {file}: schema: ")), "{err}");
    assert!(err.contains("1.9") && err.contains("1.0"), "{err}");
    let (err, _) = refused_file("no-schema", "jobs: 2\n");
    assert!(err.contains("missing key 'schema'"), "{err}");
}

#[test]
fn both_names_in_one_folder_name_both_paths() {
    let dir = repo("both-names");
    let plain = dir.0.join("repo/boxt.yaml");
    let hidden = dir.0.join("repo/.boxt.yaml");
    write(&plain, "schema: \"1.0\"\n");
    write(&hidden, "schema: \"1.0\"\n");
    let err = refused(&dir, &[]);
    assert!(
        err.starts_with(&format!("boxt: {} and {}: ", shown(&plain), shown(&hidden))),
        "{err}"
    );
}

#[test]
fn a_missing_flagged_file_is_named() {
    let dir = repo("flag-missing");
    let err = refused(&dir, &["--project".as_ref(), "nope".as_ref()]);
    assert!(
        err.starts_with("boxt: nope: cannot read the project file: "),
        "{err}"
    );
}

#[test]
fn an_unknown_target_in_the_file_lists_the_known_targets() {
    let (err, file) = refused_file("unknown-target", "schema: \"1.0\"\ntargets: [nope]\n");
    assert!(
        err.starts_with(&format!(
            "boxt: {file}: targets: unknown target nope; known targets:"
        )),
        "{err}"
    );
    assert!(err.contains("docker-default"), "{err}");
}

#[test]
fn a_file_over_1_mib_or_not_utf_8_is_refused() {
    let mut big = b"schema: \"1.0\"\n".to_vec();
    big.resize(1024 * 1024 + 1, b'\n');
    let (err, file) = refused_file("too-large", big);
    assert_eq!(
        err,
        format!("boxt: {file}: the file is 1048577 bytes, over the 1048576-byte limit\n")
    );
    let (err, file) = refused_file("not-utf8", b"schema: \"1.0\"\njobs: \xff\n");
    assert_eq!(err, format!("boxt: {file}: the file is not UTF-8\n"));
}

#[test]
fn a_merged_limit_conflict_exits_2_before_any_worker_starts() {
    let dir = repo("limit-conflict");
    write(
        &dir.0.join("repo/boxt.yaml"),
        "schema: \"1.0\"\nlimits: {memory: 1073741824}\n",
    );
    let err = refused(&dir, &[]);
    assert!(err.starts_with("boxt: limit worker-memory-cap"), "{err}");
    std::fs::remove_file(dir.0.join("repo/src/boxt-fixture-spin.c")).unwrap();
    let ok_now = boxt_in(
        &dir.0,
        [
            dir.0.join("repo/src").as_os_str(),
            "--limit".as_ref(),
            "worker-memory-cap=8589934592".as_ref(),
        ],
    );
    assert_eq!(ok_now.status.code(), Some(0), "{}", stderr(&ok_now));
}
