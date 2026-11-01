use std::ffi::OsString;
use std::num::NonZeroU32;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use boxt_collector::path::text_from_path;
use boxt_collector::walk::walk;
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{CapAllocator, Collected, RunError, RunOptions, run_with};
use boxt_core::fact::FileKind;
use boxt_core::limits::{LimitName, Limits};
use boxt_core::message::CollectedFile;
use boxt_core::outcome::{Details, FileOutcome, LinkTarget, NotCheckedReason, SpecialKind};
use boxt_core::settings::Settings;

#[global_allocator]
static ALLOCATOR: CapAllocator = CapAllocator;

fn main() -> ExitCode {
    boxt_collector::os::suppress_error_dialogs();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(WORKER_SUBCOMMAND) {
        return boxt_collector::worker::run();
    }
    let filter: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let mut tests: Vec<(&str, fn())> = vec![
        ("plain_tree", plain_tree),
        (
            "links_are_listed_and_never_read",
            links_are_listed_and_never_read,
        ),
        ("extensionless_c_is_read", extensionless_c_is_read),
        ("fake_c_is_no_reader", fake_c_is_no_reader),
        ("over_file_size_is_measured", over_file_size_is_measured),
        ("nothing_read_is_an_error", nothing_read_is_an_error),
        (
            "the_macro_table_reaches_the_source_job",
            the_macro_table_reaches_the_source_job,
        ),
        (
            "cut_is_canonical_however_workers_finish",
            cut_is_canonical_however_workers_finish,
        ),
        (
            "the_output_is_the_same_for_one_worker",
            the_output_is_the_same_for_one_worker,
        ),
        ("a_file_input_is_one_file", a_file_input_is_one_file),
        ("cut_spans_inputs_and_kinds", cut_spans_inputs_and_kinds),
        (
            "dropped_definitions_are_noted",
            dropped_definitions_are_noted,
        ),
    ];
    #[cfg(unix)]
    {
        tests.push(("fifo_does_not_block_the_walk", fifo_does_not_block_the_walk));
        tests.push((
            "a_linked_file_input_is_followed_once",
            a_linked_file_input_is_followed_once,
        ));
        tests.push((
            "an_unlistable_directory_is_unreadable",
            an_unlistable_directory_is_unreadable,
        ));
    }
    #[cfg(windows)]
    tests.push(("junction_is_a_reparse_point", junction_is_a_reparse_point));
    #[cfg(feature = "fault-fixtures")]
    {
        tests.push(("crash_ends_only_its_file", crash_ends_only_its_file));
        tests.push((
            "run_watchdog_aggregates_the_rest",
            run_watchdog_aggregates_the_rest,
        ));
        tests.push((
            "a_definitions_watchdog_marks_the_tree",
            a_definitions_watchdog_marks_the_tree,
        ));
        tests.push((
            "run_watchdog_in_the_definitions_phase",
            run_watchdog_in_the_definitions_phase,
        ));
        tests.push((
            "a_second_fault_resumes_to_an_aggregate",
            a_second_fault_resumes_to_an_aggregate,
        ));
    }
    let mut failed = 0;
    let mut ran = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        let start = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(test));
        let status = if result.is_ok() { "ok" } else { "FAILED" };
        println!(
            "test {name} ... {status} ({:.2}s)",
            start.elapsed().as_secs_f64()
        );
        failed += usize::from(result.is_err());
    }
    println!("{ran} run, {failed} failed");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct Tree(PathBuf);

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tree {
    fn new(name: &str) -> Tree {
        let root = std::env::temp_dir().join(format!("boxt-collect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tree(root)
    }

    fn file(&self, name: &str, body: &[u8]) -> &Tree {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
        self
    }

    fn listing(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut dirs = vec![self.0.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let kind = entry.file_type().unwrap();
                if kind.is_dir() {
                    dirs.push(entry.path());
                }
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }
}

fn c_body(i: usize) -> Vec<u8> {
    format!("int f{i}(int x) {{\n  return x + {i};\n}}\n").into_bytes()
}

fn settings(inputs: &[&Path], limits: Limits, jobs: Option<u32>) -> Settings {
    Settings {
        inputs: inputs.iter().map(|p| text_from_path(p)).collect(),
        targets: Default::default(),
        limits,
        encodings: Vec::new(),
        excludes: Vec::new(),
        ignored_macros: Default::default(),
        jobs: jobs.and_then(NonZeroU32::new),
        compile_database: None,
    }
}

fn options() -> RunOptions {
    RunOptions {
        program: std::env::current_exe().unwrap(),
        args: vec![OsString::from(WORKER_SUBCOMMAND)],
        env: Vec::new(),
        measure: false,
    }
}

fn collect(settings: &Settings) -> Result<Collected, RunError> {
    run_with(settings, &options()).map(|(c, _)| c)
}

fn collect_tree(tree: &Tree, limits: Limits, jobs: Option<u32>) -> Collected {
    let before = tree.listing();
    let collected = collect(&settings(&[&tree.0], limits, jobs)).unwrap();
    assert_eq!(tree.listing(), before, "the run wrote into its input");
    assert_exactly_one_outcome(tree, &collected);
    collected
}

fn assert_exactly_one_outcome(tree: &Tree, collected: &Collected) {
    let keys: Vec<_> = collected.files.iter().map(|f| f.key.clone()).collect();
    let mut sorted = keys.clone();
    sorted.dedup();
    assert_eq!(keys, sorted, "{:?}", tree.0);
    let walked = walk(&[text_from_path(&tree.0)]).unwrap();
    let paths: Vec<String> = walked[0]
        .items
        .iter()
        .map(|i| String::from_utf8_lossy(i.path().as_bytes()).into_owned())
        .collect();
    let with_outcome: Vec<String> = collected.files.iter().map(path_of).collect();
    let uncovered = paths.iter().filter(|p| !with_outcome.contains(p)).count() as u64;
    let covered_by_aggregates: u64 = collected
        .files
        .iter()
        .map(|f| match f.outcome.file {
            FileOutcome::Aggregate { count: Some(n), .. } => n - 1,
            _ => 0,
        })
        .sum();
    assert_eq!(
        uncovered, covered_by_aggregates,
        "a walked item has no outcome"
    );
    for path in &with_outcome {
        assert!(paths.contains(path), "{path} was never walked");
    }
}

fn path_of(file: &CollectedFile) -> String {
    String::from_utf8_lossy(file.key.job_path.as_bytes()).into_owned()
}

fn get<'a>(collected: &'a Collected, path: &str) -> &'a CollectedFile {
    let found: Vec<&CollectedFile> = collected
        .files
        .iter()
        .filter(|f| path_of(f) == path)
        .collect();
    assert_eq!(found.len(), 1, "{path}: {:?}", collected.files);
    found[0]
}

fn reason(file: &CollectedFile) -> Option<NotCheckedReason> {
    match file.outcome.file {
        FileOutcome::NotChecked { reason, .. } | FileOutcome::Aggregate { reason, .. } => {
            Some(reason)
        }
        _ => None,
    }
}

fn plain_tree() {
    let tree = Tree::new("plain");
    tree.file("b.c", &c_body(1))
        .file("README", b"Build it with make, then run it.\n")
        .file("a.c", &c_body(0))
        .file("sub/c.c", &c_body(2));
    let collected = collect_tree(&tree, Limits::default(), None);
    let paths: Vec<String> = collected.files.iter().map(path_of).collect();
    assert_eq!(paths, ["README", "a.c", "b.c", "sub/c.c"]);
    for path in ["a.c", "b.c", "sub/c.c"] {
        let file = get(&collected, path);
        assert_eq!(file.outcome.file, FileOutcome::Read, "{path}");
        assert_eq!(file.outcome.file_kind, Some(FileKind::Source));
        assert_eq!(file.outcome.lines.map(|l| l.lines), Some(3));
    }
    assert_eq!(
        reason(get(&collected, "README")),
        Some(NotCheckedReason::NoReader)
    );
    assert_eq!(collected.facts.len(), 0);
}

#[cfg(unix)]
fn links_are_listed_and_never_read() {
    use std::os::unix::fs::symlink;
    let tree = Tree::new("links");
    tree.file("real.c", &c_body(0));
    symlink("real.c", tree.0.join("a.c")).unwrap();
    symlink("x", tree.0.join("x")).unwrap();
    let collected = collect_tree(&tree, Limits::default(), None);
    assert_eq!(collected.files.len(), 3);
    let link = get(&collected, "a.c");
    assert_eq!(
        reason(link),
        Some(NotCheckedReason::Symlink(LinkTarget::File))
    );
    assert_eq!(link.outcome.file_kind, None);
    assert_eq!(get(&collected, "real.c").outcome.file, FileOutcome::Read);
    assert_eq!(
        reason(get(&collected, "x")),
        Some(NotCheckedReason::Symlink(LinkTarget::Loop))
    );
}

#[cfg(not(unix))]
fn links_are_listed_and_never_read() {}

#[cfg(unix)]
fn fifo_does_not_block_the_walk() {
    let tree = Tree::new("fifo");
    tree.file("a.c", &c_body(0));
    rustix::fs::mkfifoat(rustix::fs::CWD, tree.0.join("p.c"), rustix::fs::Mode::RUSR).unwrap();
    let collected = collect_tree(&tree, Limits::default(), None);
    assert_eq!(
        reason(get(&collected, "p.c")),
        Some(NotCheckedReason::SpecialFile(SpecialKind::Fifo))
    );
    assert_eq!(get(&collected, "a.c").outcome.file, FileOutcome::Read);
}

#[cfg(windows)]
fn junction_is_a_reparse_point() {
    let tree = Tree::new("junction");
    tree.file("a.c", &c_body(0)).file("target/b.c", &c_body(1));
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(tree.0.join("j"))
        .arg(tree.0.join("target"))
        .status()
        .unwrap();
    assert!(status.success());
    let collected = collect_tree(&tree, Limits::default(), None);
    assert_eq!(
        reason(get(&collected, "j")),
        Some(NotCheckedReason::SpecialFile(SpecialKind::ReparsePoint))
    );
    assert_eq!(
        get(&collected, "target/b.c").outcome.file,
        FileOutcome::Read
    );
    assert!(
        collected
            .files
            .iter()
            .all(|f| !path_of(f).starts_with("j/"))
    );
}

fn extensionless_c_is_read() {
    let tree = Tree::new("extensionless");
    tree.file(
        "config",
        b"#include <stdio.h>\n\nint main(void)\n{\n  return puts(\"hi\");\n}\n",
    );
    let collected = collect_tree(&tree, Limits::default(), None);
    let file = get(&collected, "config");
    assert_eq!(file.outcome.file, FileOutcome::Read);
    assert_eq!(file.outcome.file_kind, Some(FileKind::Source));
    assert!(!collected.facts.is_empty());
}

fn fake_c_is_no_reader() {
    let tree = Tree::new("fake");
    tree.file("a.c", &c_body(0))
        .file("fake.c", b"\x7fELF\0\0\0\x01\x02\0\0int f;\0");
    let collected = collect_tree(&tree, Limits::default(), None);
    let fake = get(&collected, "fake.c");
    assert_eq!(reason(fake), Some(NotCheckedReason::NoReader));
    assert_eq!(fake.outcome.file_kind, None);
}

fn over_file_size_is_measured() {
    let tree = Tree::new("file-size");
    let mut body = Vec::new();
    while body.len() < 1000 {
        body.extend_from_slice(b"int padding;\n");
    }
    tree.file("a.c", &c_body(0)).file("big.c", &body);
    let limits = Limits::default().with(LimitName::FileSize, 500).unwrap();
    let collected = collect_tree(&tree, limits, None);
    let big = get(&collected, "big.c");
    assert_eq!(
        big.outcome.file,
        FileOutcome::NotChecked {
            reason: NotCheckedReason::LimitExceeded(LimitName::FileSize),
            details: Details::Measured {
                value: body.len() as u64,
                limit: 500
            },
        }
    );
    assert_eq!(big.outcome.lines, None);
}

fn nothing_read_is_an_error() {
    let tree = Tree::new("nothing");
    tree.file("README", b"Nothing here.\n")
        .file("docs/README", b"Nor here.\n");
    assert_eq!(
        collect(&settings(&[&tree.0], Limits::default(), None)),
        Err(RunError::NothingRead)
    );
}

fn the_macro_table_reaches_the_source_job() {
    let tree = Tree::new("macros");
    tree.file(
        "ns.h",
        b"#define NS_BEGIN namespace foo {\n#define NS_END }\n#define DECL(t, n) t n\n",
    )
    .file(
        "a.cpp",
        b"#include \"ns.h\"\nNS_BEGIN\nDECL(int, x) = 1;\nint f(int a) { return g(a); }\nNS_END\nint h(void) { return x; }\n",
    );
    let with = collect_tree(&tree, Limits::default(), None);
    let source = get(&with, "a.cpp");
    assert_eq!(source.outcome.file, FileOutcome::Read);
    assert!(source.regions.is_empty(), "{:?}", source.regions);
    assert!(with.macro_tables_cut.is_empty());

    let cut = Limits::default()
        .with(LimitName::MacroTableBytes, 1)
        .unwrap();
    let without = collect_tree(&tree, cut, None);
    assert!(!get(&without, "a.cpp").regions.is_empty());
    assert_eq!(without.macro_tables_cut.as_slice(), &[0]);
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn cut_is_canonical_however_workers_finish() {
    let limits = Limits::default().with(LimitName::TotalEntries, 30).unwrap();
    let mut first: Option<Vec<String>> = None;
    for run in 0..10u64 {
        let tree = Tree::new(&format!("cut-{run}"));
        let mut rng = Lcg(run.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for i in 0..50 {
            let mut body = Vec::new();
            for k in 0..(rng.next() % 400) {
                body.extend_from_slice(format!("int v{i}_{k} = {k};\n").as_bytes());
            }
            body.extend_from_slice(&c_body(i));
            tree.file(&format!("f{i:02}.c"), &body);
        }
        let collected = collect_tree(&tree, limits.clone(), Some(4));
        let read: Vec<String> = collected
            .files
            .iter()
            .filter(|f| f.outcome.file == FileOutcome::Read)
            .map(path_of)
            .collect();
        let expect: Vec<String> = (0..30).map(|i| format!("f{i:02}.c")).collect();
        assert_eq!(read, expect, "run {run}");
        let aggregates: Vec<&CollectedFile> = collected
            .files
            .iter()
            .filter(|f| matches!(f.outcome.file, FileOutcome::Aggregate { .. }))
            .collect();
        assert_eq!(aggregates.len(), 1, "one aggregate for the one FileKind");
        assert_eq!(
            aggregates[0].outcome.file,
            FileOutcome::Aggregate {
                reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
                count: Some(20),
                at_least: 20,
            }
        );
        assert_eq!(aggregates[0].outcome.file_kind, Some(FileKind::Source));
        assert_eq!(path_of(aggregates[0]), "f30.c");
        assert_eq!(collected.files.len(), 31);
        match &first {
            None => first = Some(read),
            Some(f) => assert_eq!(f, &read),
        }
    }
}

fn the_output_is_the_same_for_one_worker() {
    let tree = Tree::new("same");
    for i in 0..12 {
        tree.file(
            &format!("d{}/f{i}.c", i % 3),
            format!("#include <x.h>\n#ifdef A\nint f{i}(void) {{ return g(\"{i}\"); }}\n#endif\n")
                .as_bytes(),
        );
    }
    tree.file("notes.txt", b"text\n");
    let many = collect_tree(&tree, Limits::default(), None);
    let one = collect_tree(&tree, Limits::default(), Some(1));
    let again = collect_tree(&tree, Limits::default(), Some(3));
    assert_eq!(many, one);
    assert_eq!(many, again);
    assert_eq!(many.digest(), one.digest());
    assert!(many.facts.len() >= 12);
}

fn a_file_input_is_one_file() {
    let tree = Tree::new("file-input");
    tree.file("a.c", &c_body(0)).file("b.c", &c_body(1));
    let collected = collect(&settings(
        &[&tree.0.join("b.c"), &tree.0],
        Limits::default(),
        None,
    ))
    .unwrap();
    let keys: Vec<(u32, String)> = collected
        .files
        .iter()
        .map(|f| (f.key.input_index, path_of(f)))
        .collect();
    assert_eq!(
        keys,
        [
            (0, "b.c".to_string()),
            (1, "a.c".to_string()),
            (1, "b.c".to_string())
        ]
    );
    assert!(
        collected
            .files
            .iter()
            .all(|f| f.outcome.file == FileOutcome::Read)
    );
}

#[cfg(feature = "fault-fixtures")]
fn crash_ends_only_its_file() {
    let tree = Tree::new("crash");
    for i in 0..20 {
        tree.file(&format!("ok-{i:02}.c"), &c_body(i));
    }
    tree.file("boxt-fixture-segfault.c", &c_body(0));
    let collected = collect_tree(&tree, Limits::default(), Some(2));
    let crashed = get(&collected, "boxt-fixture-segfault.c");
    assert_eq!(
        crashed.outcome.file,
        FileOutcome::NotChecked {
            reason: NotCheckedReason::Fault(boxt_core::outcome::FaultKind::Crash),
            details: Details::None,
        }
    );
    let read = collected
        .files
        .iter()
        .filter(|f| f.outcome.file == FileOutcome::Read)
        .count();
    assert_eq!(read, 20);
    assert_eq!(collected.files.len(), 21);
}

#[cfg(feature = "fault-fixtures")]
fn run_watchdog_aggregates_the_rest() {
    let tree = Tree::new("run-watchdog");
    for i in 0..10 {
        tree.file(&format!("a{i}.c"), &c_body(i));
        tree.file(&format!("z{i}.c"), &c_body(i));
    }
    tree.file("m-notes", b"Some notes.\n");
    tree.file("boxt-fixture-spin.c", &c_body(0));
    let limits = Limits::default().with(LimitName::RunWatchdog, 1).unwrap();
    let start = Instant::now();
    let collected = collect_tree(&tree, limits, Some(1));
    assert!(start.elapsed().as_secs_f64() < 30.0);
    let aggregates: Vec<&CollectedFile> = collected
        .files
        .iter()
        .filter(|f| matches!(f.outcome.file, FileOutcome::Aggregate { .. }))
        .collect();
    assert_eq!(aggregates.len(), 2, "{aggregates:?}");
    for aggregate in &aggregates {
        assert!(aggregate.outcome.host_dependent);
        assert_eq!(
            reason(aggregate),
            Some(NotCheckedReason::LimitExceeded(LimitName::RunWatchdog))
        );
    }
    let source = aggregates
        .iter()
        .find(|a| a.outcome.file_kind == Some(FileKind::Source))
        .unwrap();
    assert_eq!(path_of(source), "boxt-fixture-spin.c");
    assert_eq!(
        source.outcome.file,
        FileOutcome::Aggregate {
            reason: NotCheckedReason::LimitExceeded(LimitName::RunWatchdog),
            count: Some(11),
            at_least: 11,
        }
    );
    for i in 0..10 {
        assert_eq!(
            get(&collected, &format!("a{i}.c")).outcome.file,
            FileOutcome::Read
        );
    }
}

#[cfg(unix)]
fn a_linked_file_input_is_followed_once() {
    let tree = Tree::new("file-link");
    tree.file("real.c", &c_body(0));
    std::os::unix::fs::symlink(tree.0.join("real.c"), tree.0.join("link.c")).unwrap();
    let collected = collect(&settings(
        &[&tree.0.join("link.c")],
        Limits::default(),
        None,
    ))
    .unwrap();
    let file = get(&collected, "link.c");
    assert_eq!(file.outcome.file, FileOutcome::Read);
    assert_eq!(file.outcome.file_kind, Some(FileKind::Source));
}

#[cfg(unix)]
fn an_unlistable_directory_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        println!("skipped: root lists every directory");
        return;
    }
    let tree = Tree::new("unlistable");
    tree.file("a.c", &c_body(0)).file("locked/b.c", &c_body(1));
    let locked = tree.0.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let collected = collect(&settings(&[&tree.0], Limits::default(), None));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let collected = collected.unwrap();
    let paths: Vec<String> = collected.files.iter().map(path_of).collect();
    assert_eq!(paths, ["a.c", "locked"]);
    assert_eq!(
        reason(get(&collected, "locked")),
        Some(NotCheckedReason::Unreadable(
            boxt_core::outcome::IoKind::PermissionDenied
        ))
    );
}

fn cut_spans_inputs_and_kinds() {
    let first = Tree::new("cut-first");
    first.file("README", b"Build it.\n");
    for i in 0..5 {
        first.file(&format!("a{i}.c"), &c_body(i));
    }
    let second = Tree::new("cut-second");
    second.file("b0.c", &c_body(0)).file("notes.txt", b"text\n");
    let limits = Limits::default().with(LimitName::TotalEntries, 3).unwrap();
    let collected = collect(&settings(&[&first.0, &second.0], limits, None)).unwrap();
    let aggregates: Vec<(u32, String, Option<FileKind>, FileOutcome)> = collected
        .files
        .iter()
        .filter(|f| matches!(f.outcome.file, FileOutcome::Aggregate { .. }))
        .map(|f| {
            (
                f.key.input_index,
                path_of(f),
                f.outcome.file_kind,
                f.outcome.file.clone(),
            )
        })
        .collect();
    let cut = |n| FileOutcome::Aggregate {
        reason: NotCheckedReason::LimitExceeded(LimitName::TotalEntries),
        count: Some(n),
        at_least: n,
    };
    assert_eq!(
        aggregates,
        [
            (0, "a2.c".to_string(), Some(FileKind::Source), cut(3)),
            (1, "b0.c".to_string(), Some(FileKind::Source), cut(1)),
            (1, "notes.txt".to_string(), None, cut(1)),
        ]
    );
    let read: Vec<String> = collected
        .files
        .iter()
        .filter(|f| f.outcome.file == FileOutcome::Read)
        .map(path_of)
        .collect();
    assert_eq!(read, ["a0.c", "a1.c"]);
}

fn dropped_definitions_are_noted() {
    let tree = Tree::new("dropped");
    tree.file(
        "a.h",
        b"#define OK 1\n#define TWICE(a, a) a\n#define VA(__VA_ARGS__) x\nint f(void);\n",
    );
    let collected = collect_tree(&tree, Limits::default(), None);
    let file = get(&collected, "a.h");
    assert_eq!(file.outcome.file, FileOutcome::Read);
    assert_eq!(
        file.notes,
        [boxt_core::message::Note::DroppedDefinitions(2)]
    );
}

#[cfg(feature = "fault-fixtures")]
fn a_definitions_watchdog_marks_the_tree() {
    let tree = Tree::new("defs-watchdog");
    tree.file("a.c", &c_body(0))
        .file("boxt-fixture-defs-spin.c", &c_body(1));
    let other = Tree::new("defs-watchdog-other");
    other.file("b.c", &c_body(2));
    let limits = Limits::default().with(LimitName::Watchdog, 1).unwrap();
    let collected = collect(&settings(&[&tree.0, &other.0], limits, Some(2))).unwrap();
    assert_eq!(collected.host_dependent_trees.as_slice(), &[0]);
    for path in ["a.c", "boxt-fixture-defs-spin.c"] {
        let file = get(&collected, path);
        assert_eq!(file.outcome.file, FileOutcome::Read, "{path}");
        assert!(file.outcome.host_dependent, "{path}");
    }
    assert!(!get(&collected, "b.c").outcome.host_dependent);
}

#[cfg(feature = "fault-fixtures")]
fn run_watchdog_in_the_definitions_phase() {
    let tree = Tree::new("defs-run-watchdog");
    tree.file("a.c", &c_body(0))
        .file("boxt-fixture-defs-spin.c", &c_body(1));
    let limits = Limits::default().with(LimitName::RunWatchdog, 1).unwrap();
    let start = Instant::now();
    assert_eq!(
        collect(&settings(&[&tree.0], limits, Some(1))),
        Err(RunError::NothingRead)
    );
    assert!(start.elapsed().as_secs_f64() < 30.0);
}

#[cfg(feature = "fault-fixtures")]
fn a_second_fault_resumes_to_an_aggregate() {
    use boxt_core::message::EntryPath;
    let tree = Tree::new("repeated");
    tree.file("a.c", &c_body(0))
        .file("boxt-fixture-container-crash.c", &c_body(1))
        .file("z.c", &c_body(2));
    let collected = collect(&settings(&[&tree.0], Limits::default(), Some(1))).unwrap();
    let entries: Vec<(EntryPath, FileOutcome)> = collected
        .files
        .iter()
        .filter(|f| path_of(f) == "boxt-fixture-container-crash.c")
        .map(|f| (f.key.entry.clone(), f.outcome.file.clone()))
        .collect();
    assert_eq!(
        entries,
        [
            (EntryPath::from([0]), FileOutcome::Read),
            (
                EntryPath::from([1]),
                FileOutcome::NotChecked {
                    reason: NotCheckedReason::Fault(boxt_core::outcome::FaultKind::Crash),
                    details: Details::None,
                }
            ),
            (
                EntryPath::from([2]),
                FileOutcome::Aggregate {
                    reason: NotCheckedReason::RepeatedFault,
                    count: None,
                    at_least: 1,
                }
            ),
        ]
    );
    assert_eq!(get(&collected, "a.c").outcome.file, FileOutcome::Read);
    assert_eq!(get(&collected, "z.c").outcome.file, FileOutcome::Read);
}
