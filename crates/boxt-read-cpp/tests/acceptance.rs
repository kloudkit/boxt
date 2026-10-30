use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use boxt_core::Text;
use boxt_core::compile_db::extension_class;
use boxt_core::decode::decode;
use boxt_core::fact::{Fact, GuardChain};
use boxt_core::limits::Limits;
use boxt_core::location::{InputRef, Location, RelPath};
use boxt_core::message::{MacroTable, Note, Region};
use boxt_core::outcome::{NotCheckedReason, line_count};
use boxt_core::read::{Exceeded, FactSink, ReadCtx, Reader};
use boxt_core::settings::Encoding;
use boxt_read_cpp::{CppReader, definitions};
use serde::Deserialize;

const ROOT_ENV: &str = "BOXT_ACCEPT_ROOT";

const CODE_BASES: &[(&str, &str)] = &[
    ("parser-spike/sources", "openssh-9.9p2"),
    ("parser-spike/sources", "ntp-4.2.8p18"),
    ("parser-spike/sources", "xerces-c-3.2.5"),
    ("parser-mitigations/sources", "net-snmp-5.9.4"),
    ("parser-mitigations/sources", "squid-6.12"),
    ("parser-mitigations-t5/sources-fresh", "sendmail-8.18.2"),
    ("parser-mitigations-t5/sources-fresh", "ace-6.5.25"),
];

const SAMPLES: &[(&str, &str)] = &[
    ("parser-spike/sample.yaml", "parser-spike/sources"),
    (
        "parser-mitigations/heldout-sample.yaml",
        "parser-mitigations/sources",
    ),
];

#[derive(Default)]
struct Sink {
    facts: Vec<Fact>,
    regions: Vec<Region>,
}

impl FactSink for Sink {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
        assert!(fact.validate().is_ok(), "{fact:?}");
        self.facts.push(fact);
        Ok(())
    }

    fn region(&mut self, region: Region) -> Result<(), Exceeded> {
        self.regions.push(region);
        Ok(())
    }

    fn note(&mut self, _: Note) -> Result<(), Exceeded> {
        Ok(())
    }
}

struct FileResult {
    declared: Option<Encoding>,
    lines: u64,
    bad: u64,
    outcome: Option<NotCheckedReason>,
    facts: Vec<Fact>,
    regions: Vec<Region>,
}

fn walk(root: &Path) -> Vec<RelPath> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            let path = entry.path();
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() {
                let rel = path.strip_prefix(root).unwrap().to_str().unwrap();
                let rel = RelPath::try_from(rel.replace('\\', "/").as_str()).unwrap();
                if extension_class(rel.file_name()).is_some() {
                    out.push(rel);
                }
            }
        }
    }
    out.sort();
    out
}

fn location(root: &Path, rel: &RelPath) -> Location {
    Location::file(
        InputRef {
            index: 0,
            given: Text::from(root.to_str().unwrap()),
        },
        Vec::new(),
        rel.clone(),
    )
}

fn declared(bytes: &[u8]) -> Option<Encoding> {
    decode(bytes, None).is_err().then_some(Encoding::Latin1)
}

fn table(root: &Path, files: &[RelPath]) -> MacroTable {
    let mut all = Vec::new();
    for rel in files {
        let bytes = std::fs::read(root.join(rel.as_text().as_str().unwrap())).unwrap();
        if let Ok(defs) = definitions(&bytes, declared(&bytes), &location(root, rel)) {
            all.extend(defs);
        }
    }
    MacroTable::from(all)
}

fn read_file(root: &Path, rel: &RelPath, table: &MacroTable) -> FileResult {
    let bytes = std::fs::read(root.join(rel.as_text().as_str().unwrap())).unwrap();
    let declared = declared(&bytes);
    let lines = line_count(&bytes, declared);
    let mut ctx =
        ReadCtx::new(location(root, rel), &Limits::default(), declared).with_macros(table);
    let mut sink = Sink::default();
    match CppReader.read(&bytes, &mut ctx, &mut sink) {
        Ok(()) => {
            let mut bits = vec![false; lines as usize + 1];
            for r in &sink.regions {
                let fit = r.fit_within(lines).expect("a region within the file");
                for l in fit.first_line..=fit.last_line {
                    bits[l as usize] = true;
                }
            }
            FileResult {
                declared,
                lines,
                bad: bits.iter().filter(|&&b| b).count() as u64,
                outcome: None,
                facts: sink.facts,
                regions: sink.regions,
            }
        }
        Err(not_read) => FileResult {
            declared,
            lines,
            bad: lines,
            outcome: Some(not_read.reason),
            facts: Vec::new(),
            regions: Vec::new(),
        },
    }
}

fn read_all(root: &Path, files: &[RelPath], table: &MacroTable) -> Vec<FileResult> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut slots: Vec<Option<FileResult>> = (0..files.len()).map(|_| None).collect();
    let done: Vec<Vec<(usize, FileResult)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(rel) = files.get(i) else {
                            break;
                        };
                        mine.push((i, read_file(root, rel, table)));
                    }
                    mine
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, result) in done.into_iter().flatten() {
        slots[i] = Some(result);
    }
    slots.into_iter().map(Option::unwrap).collect()
}

fn digest(files: &[RelPath], results: &[FileResult]) -> u64 {
    let mut h = DefaultHasher::new();
    for (rel, r) in files.iter().zip(results) {
        h.write(rel.as_bytes());
        h.write(serde_json::to_string(&r.outcome).unwrap().as_bytes());
        h.write(serde_json::to_string(&r.facts).unwrap().as_bytes());
        h.write(serde_json::to_string(&r.regions).unwrap().as_bytes());
    }
    h.finish()
}

#[derive(Deserialize, Clone, PartialEq, Debug)]
struct SFrame {
    directive: String,
    text: String,
    holds_code: bool,
}

#[derive(Deserialize, Clone, PartialEq, Debug)]
struct SChain {
    has_alternative: bool,
    frames: Vec<SFrame>,
}

#[derive(Deserialize)]
struct Entry {
    id: u32,
    codebase: String,
    file: String,
    line: u32,
    callee: String,
    #[serde(default)]
    stack: Option<Vec<SChain>>,
    #[serde(default)]
    unreadable: Option<String>,
}

#[derive(Deserialize)]
struct Sample {
    entries: Vec<Entry>,
}

fn chains(guards: &[GuardChain]) -> Vec<SChain> {
    guards
        .iter()
        .map(|c| SChain {
            has_alternative: c.has_alternative,
            frames: c
                .frames
                .iter()
                .map(|f| SFrame {
                    directive: format!("{:?}", f.directive),
                    text: f.text.as_str().unwrap().to_string(),
                    holds_code: f.holds_code,
                })
                .collect(),
        })
        .collect()
}

fn squash(t: &str) -> String {
    t.split_whitespace().collect()
}

#[derive(Default, Debug)]
struct Tally {
    matched: u32,
    mismatched: u32,
    skipped: u32,
    unreadable_reported: u32,
    unreadable_resolved: u32,
}

fn check_sample(
    accept: &Path,
    sample: &str,
    sources: &str,
    tables: &[(String, MacroTable)],
    report: &mut String,
) -> Tally {
    let text = std::fs::read_to_string(accept.join(sample)).unwrap();
    let sample: Sample = serde_saphyr::from_str(&text).unwrap();
    let mut tally = Tally::default();
    for e in &sample.entries {
        let root = accept.join(sources).join(&e.codebase);
        let table = &tables.iter().find(|(n, _)| *n == e.codebase).unwrap().1;
        let rel = RelPath::try_from(e.file.as_str()).unwrap();
        let r = read_file(&root, &rel, table);
        let reported = r.outcome.is_some()
            || r.regions
                .iter()
                .any(|g| g.first_line <= e.line && e.line <= g.last_line);
        let found: Vec<Vec<SChain>> = r
            .facts
            .iter()
            .filter_map(|f| match f {
                Fact::Call {
                    location,
                    callee,
                    guards,
                    ..
                } if location.line == Some(e.line)
                    && squash(callee.as_str().unwrap_or("")) == e.callee =>
                {
                    Some(chains(guards))
                }
                _ => None,
            })
            .collect();
        let tag = format!("#{} {}:{} {}", e.id, e.codebase, e.file, e.line);
        if let Some(range) = &e.unreadable {
            if reported {
                tally.unreadable_reported += 1;
            } else if !found.is_empty() {
                tally.unreadable_resolved += 1;
                report.push_str(&format!(
                    "  resolved  {tag} labelled unreadable {range}, now read\n"
                ));
            } else {
                tally.mismatched += 1;
                report.push_str(&format!(
                    "  MISMATCH  {tag} labelled unreadable {range}, neither reported nor read\n"
                ));
            }
            continue;
        }
        if reported {
            tally.skipped += 1;
            report.push_str(&format!("  skipped   {tag} lies in a reported range\n"));
            continue;
        }
        let want = e.stack.clone().unwrap_or_default();
        if found.contains(&want) {
            tally.matched += 1;
        } else {
            tally.mismatched += 1;
            report.push_str(&format!(
                "  MISMATCH  {tag} {}\n    expected {want:?}\n    got      {found:?}\n",
                e.callee
            ));
        }
    }
    tally
}

#[test]
#[ignore = "needs the seven spike code bases; run with BOXT_ACCEPT_ROOT set"]
fn ad13_bar_on_the_seven_code_bases() {
    let accept = PathBuf::from(
        std::env::var_os(ROOT_ENV).expect("BOXT_ACCEPT_ROOT names the spikes directory"),
    );
    let mut report = String::new();
    let mut failures = Vec::new();
    let mut tables: Vec<(String, MacroTable)> = Vec::new();
    for (dir, name) in CODE_BASES {
        let root = accept.join(dir).join(name);
        let files = walk(&root);
        let table = table(&root, &files);
        let first = read_all(&root, &files, &table);
        let second = read_all(&root, &files, &table);
        let (d1, d2) = (digest(&files, &first), digest(&files, &second));
        let lines: u64 = first.iter().map(|r| r.lines).sum();
        let bad: u64 = first.iter().map(|r| r.bad).sum();
        let not_checked: Vec<(&RelPath, NotCheckedReason, u64)> = files
            .iter()
            .zip(&first)
            .filter_map(|(f, r)| r.outcome.map(|o| (f, o, r.lines)))
            .collect();
        let partial = first
            .iter()
            .filter(|r| r.outcome.is_none() && !r.regions.is_empty())
            .count();
        let facts: usize = first.iter().map(|r| r.facts.len()).sum();
        let latin: Vec<(&RelPath, u64)> = files
            .iter()
            .zip(&first)
            .filter(|(_, r)| r.declared.is_some())
            .map(|(f, r)| (f, r.lines))
            .collect();
        let latin_lines: u64 = latin.iter().map(|(_, l)| l).sum();
        report.push_str(&format!(
            "{name}: files {}, declared latin1 {} ({latin_lines} lines; undeclared they would be not checked: {:.5}), not checked {}, read with regions {partial}, unreadable lines {bad}/{lines} = {:.5}, facts {facts}, macros {}, digest {d1:016x} / {d2:016x}\n",
            files.len(),
            latin.len(),
            (bad + latin_lines) as f64 / lines.max(1) as f64,
            not_checked.len(),
            bad as f64 / lines.max(1) as f64,
            table.len(),
        ));
        for (f, reason, l) in &not_checked {
            report.push_str(&format!(
                "  not checked {} ({l} lines): {reason:?}\n",
                f.as_text().as_str().unwrap()
            ));
        }
        if bad.saturating_mul(100) > lines {
            failures.push(format!("{name}: line share {bad}/{lines} over 1/100"));
        }
        if d1 != d2 {
            failures.push(format!("{name}: two runs differ"));
        }
        tables.push((name.to_string(), table));
    }
    for (sample, sources) in SAMPLES {
        let mut detail = String::new();
        let tally = check_sample(&accept, sample, sources, &tables, &mut detail);
        report.push_str(&format!("sample {sample}: {tally:?}\n{detail}"));
        if tally.mismatched > 0 {
            failures.push(format!("{sample}: {} guard mismatches", tally.mismatched));
        }
    }
    println!("{report}");
    assert!(failures.is_empty(), "{failures:#?}");
}
