use std::io::Write;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use boxt_collector::CapAllocator;
use boxt_collector::bench::{cxx_jobs, measure_reads, parse_in_process};
use boxt_collector::boxt_core::fact::Fact;
use boxt_collector::boxt_core::limits::{LimitName, Limits};
use boxt_collector::boxt_core::outcome::{FileOutcome, sm3_share, verdict_counts};
use boxt_collector::boxt_core::settings::Settings;
use boxt_collector::boxt_core::verdict::TargetId;
use boxt_collector::boxt_core::{SortedVec, Text};
use boxt_collector::path::text_from_path;
use boxt_collector::pool::{self, PoolConfig, default_jobs};
use boxt_collector::worker::WORKER_SUBCOMMAND;
use boxt_collector::{Collected, RunOptions, RunStats};
use boxt_rules::{embedded, evaluate, load_tar};
use boxt_verdict::{VerdictInput, build, resolve_targets};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

#[global_allocator]
static ALLOCATOR: CapAllocator = CapAllocator;

const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(
    name = "boxt",
    version,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(value_name = "INPUT", required = true)]
    inputs: Vec<PathBuf>,
    #[arg(long = "target", value_name = "ID")]
    targets: Vec<String>,
    #[arg(long)]
    jobs: Option<NonZeroU32>,
    #[arg(long = "limit", value_name = "NAME=VALUE")]
    limits: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    #[command(name = WORKER_SUBCOMMAND, hide = true)]
    Worker,
    #[command(name = "__bench", hide = true)]
    Bench {
        root: PathBuf,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..))]
        runs: u32,
        #[arg(long)]
        jobs: Option<NonZeroU32>,
    },
    #[command(name = "__measure", hide = true)]
    Measure {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        jobs: Option<NonZeroU32>,
    },
    #[command(name = "__collect", hide = true)]
    Collect {
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        #[arg(long)]
        jobs: Option<NonZeroU32>,
        #[arg(long = "limit", value_name = "NAME=VALUE")]
        limits: Vec<String>,
        #[arg(long)]
        measure: bool,
        #[arg(long)]
        list: bool,
    },
}

fn main() -> ExitCode {
    boxt_collector::os::suppress_error_dialogs();
    std::panic::set_hook(Box::new(|info| {
        let _ = writeln!(std::io::stderr(), "boxt: internal error: {info}");
        std::process::exit(i32::from(EXIT_ERROR));
    }));
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let rendered = e.render().to_string();
            eprint!(
                "boxt: {}",
                rendered.strip_prefix("error: ").unwrap_or(&rendered)
            );
            return ExitCode::from(EXIT_ERROR);
        }
    };
    match cli.command {
        Some(Command::Worker) => {
            drop(std::panic::take_hook());
            boxt_collector::worker::run()
        }
        Some(Command::Bench { root, runs, jobs }) => bench(&root, runs, jobs),
        Some(Command::Measure { inputs, jobs }) => measure(&inputs, jobs),
        Some(Command::Collect {
            inputs,
            jobs,
            limits,
            measure,
            list,
        }) => collect(&inputs, jobs, &limits, measure, list),
        None => match report(&cli.inputs, &cli.targets, cli.jobs, &cli.limits) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("boxt: {e}");
                ExitCode::from(EXIT_ERROR)
            }
        },
    }
}

fn settings(
    inputs: &[PathBuf],
    targets: &[String],
    jobs: Option<NonZeroU32>,
    limits: &[String],
) -> Result<Settings, String> {
    let targets = targets
        .iter()
        .map(|t| TargetId::new(t.as_str()).map_err(|e| format!("--target {t}: {e}")))
        .collect::<Result<SortedVec<_>, _>>()?;
    Ok(Settings {
        inputs: inputs.iter().map(|p| text_from_path(p)).collect(),
        targets,
        limits: parse_limits(limits)?,
        encodings: Vec::new(),
        excludes: Vec::new(),
        ignored_macros: Default::default(),
        jobs,
        compile_database: None,
    })
}

fn report(
    inputs: &[PathBuf],
    targets: &[String],
    jobs: Option<NonZeroU32>,
    limits: &[String],
) -> Result<(), String> {
    let settings = settings(inputs, targets, jobs, limits)?;
    #[cfg(feature = "fault-fixtures")]
    if inputs.iter().any(|p| {
        p.file_name()
            .is_some_and(|n| n == "boxt-fixture-main-panic")
    }) {
        panic!("fixture panic in the main process");
    }
    let roots = inputs
        .iter()
        .map(|p| std::path::absolute(p).map(|a| text_from_path(&a)))
        .collect::<Result<Vec<Text>, _>>()
        .map_err(|e| format!("cannot make an input path absolute: {e}"))?;
    let corpus = load_tar(embedded()).map_err(|e| e.to_string())?;
    resolve_targets(&settings.targets, &corpus.profiles).map_err(|e| e.to_string())?;
    let collected = boxt_collector::run(&settings).map_err(|e| e.to_string())?;
    let facts: Vec<Fact> = collected
        .facts
        .into_iter()
        .flat_map(|file| file.facts)
        .collect();
    let matched = evaluate(&corpus, &facts);
    let result = build(&VerdictInput {
        targets: &settings.targets,
        profiles: &corpus.profiles,
        files: &collected.files,
        findings: &matched.findings,
        observations: &matched.observations,
        macro_tables_cut: &collected.macro_tables_cut,
        host_dependent_trees: &collected.host_dependent_trees,
        roots: &roots,
        compile_database_used: false,
    })
    .map_err(|e| e.to_string())?;
    let stdout = std::io::stdout();
    let styled = anstream::AutoStream::choice(&stdout) != anstream::ColorChoice::Never;
    let text = boxt_render::terminal(&result, styled);
    let mut out = anstream::AutoStream::auto(stdout.lock());
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| format!("cannot write the report: {e}"))
}

fn parse_limits(given: &[String]) -> Result<Limits, String> {
    let mut limits = Limits::default();
    for spec in given {
        let (name, value) = spec
            .split_once('=')
            .ok_or_else(|| format!("--limit {spec}: expected NAME=VALUE"))?;
        let limit = LimitName::from_flag(name).ok_or_else(|| format!("unknown limit {name}"))?;
        let value = value
            .parse::<u64>()
            .map_err(|_| format!("--limit {spec}: not a whole number"))?;
        limits.set(limit, value).map_err(|e| e.to_string())?;
    }
    limits.validate().map_err(|e| e.to_string())?;
    Ok(limits)
}

fn collect(
    inputs: &[PathBuf],
    jobs: Option<NonZeroU32>,
    limits: &[String],
    measure: bool,
    list: bool,
) -> ExitCode {
    let limits = match parse_limits(limits) {
        Ok(limits) => limits,
        Err(e) => {
            eprintln!("boxt: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let settings = Settings {
        inputs: inputs.iter().map(|p| text_from_path(p)).collect(),
        targets: SortedVec::new(),
        limits,
        encodings: Vec::new(),
        excludes: Vec::new(),
        ignored_macros: Default::default(),
        jobs,
        compile_database: None,
    };
    let mut options = match RunOptions::current() {
        Ok(options) => options,
        Err(e) => {
            eprintln!("boxt: cannot find its own executable: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    options.measure = measure;
    let (collected, stats) = match boxt_collector::run_with(&settings, &options) {
        Ok(done) => done,
        Err(e) => {
            eprintln!("boxt: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    if list {
        for file in collected.files.iter() {
            if file.outcome.file != FileOutcome::Read || !file.regions.is_empty() {
                println!(
                    "{} {:?} {:?} lines {:?} regions {}",
                    String::from_utf8_lossy(file.key.job_path.as_bytes()),
                    file.outcome.file_kind,
                    file.outcome.file,
                    file.outcome.lines.map(|l| l.lines),
                    file.regions.len()
                );
            }
        }
    }
    print_counts(&collected);
    if measure {
        print_measure(&collected, &stats);
    }
    ExitCode::SUCCESS
}

fn print_counts(collected: &Collected) {
    let files = &collected.files;
    let counts = verdict_counts(files.iter().map(|f| &f.outcome.file));
    let aggregates = files
        .iter()
        .filter(|f| matches!(f.outcome.file, FileOutcome::Aggregate { .. }))
        .count();
    let host = files.iter().filter(|f| f.outcome.host_dependent).count();
    let facts: usize = collected.facts.iter().map(|f| f.facts.len()).sum();
    let sm3 = sm3_share(files.iter());
    let lines = sm3.lines.map_or("n/a".to_string(), |l| {
        format!(
            "{}{}/{}",
            if l.is_lower_bound() { "at least " } else { "" },
            l.numerator,
            l.denominator
        )
    });
    let shares = sm3.files.map_or("n/a".to_string(), |f| {
        format!("{}/{}", f.numerator, f.denominator)
    });
    println!(
        "outcomes {} read {} not-checked {} aggregates {aggregates} host-dependent {host} facts {facts} sm3-lines {lines} sm3-files {shares} tables-cut {:?} digest {:016x}",
        files.len(),
        counts.read,
        counts.not_checked,
        collected.macro_tables_cut.as_slice(),
        collected.digest(),
    );
}

fn print_measure(collected: &Collected, stats: &RunStats) {
    let cut = |reason| -> u64 {
        collected
            .files
            .iter()
            .map(|f| match f.outcome.file {
                FileOutcome::NotChecked { reason: r, .. } if r == reason => 1,
                FileOutcome::Aggregate {
                    reason: r,
                    count,
                    at_least,
                } if r == reason => count.unwrap_or(at_least.max(1)),
                _ => 0,
            })
            .sum()
    };
    use boxt_collector::boxt_core::outcome::NotCheckedReason::LimitExceeded as Over;
    println!(
        "measure entries {} largest-file {} longest-job-ms {} run-ms {} worker-peak-bytes {} macro-table-bytes {} most-entry-facts {}",
        stats.walked,
        stats.largest_file,
        stats.longest_job.as_millis(),
        stats.elapsed.as_millis(),
        stats.worker_peak_bytes,
        stats.macro_table_bytes,
        stats.most_entry_facts,
    );
    println!(
        "cut-files file-size {} work {} memory {} syntax-depth {} entry-facts {} entry-message-bytes {} watchdog {} worker-memory-cap {} total-entries {} run-watchdog {} macro-table-bytes-trees {}",
        cut(Over(LimitName::FileSize)),
        cut(Over(LimitName::Work)),
        cut(Over(LimitName::Memory)),
        cut(Over(LimitName::SyntaxDepth)),
        cut(Over(LimitName::EntryFacts)),
        cut(Over(LimitName::EntryMessageBytes)),
        cut(Over(LimitName::Watchdog)),
        cut(Over(LimitName::WorkerMemoryCap)),
        cut(Over(LimitName::TotalEntries)),
        cut(Over(LimitName::RunWatchdog)),
        collected.macro_tables_cut.len(),
    );
}

fn measure(inputs: &[PathBuf], jobs: Option<NonZeroU32>) -> ExitCode {
    let raised = Limits::default()
        .with(LimitName::Work, u64::MAX)
        .and_then(|l| l.with(LimitName::Memory, u64::MAX / 8))
        .and_then(|l| l.with(LimitName::WorkerMemoryCap, u64::MAX))
        .and_then(|l| l.with(LimitName::SyntaxDepth, u64::MAX))
        .and_then(|l| l.with(LimitName::MacroTableBytes, u64::MAX))
        .and_then(|l| l.with(LimitName::FileSize, u64::MAX))
        .and_then(|l| l.with(LimitName::EntryFacts, u64::MAX));
    let inputs: Vec<_> = inputs.iter().map(|p| text_from_path(p)).collect();
    let peaks = raised
        .map_err(|e| std::io::Error::other(e.to_string()))
        .and_then(|raised| measure_reads(&inputs, &raised, &Limits::default(), default_jobs(jobs)));
    match peaks {
        Ok(p) => {
            println!(
                "in-process candidates {} read-as-cxx {} not-cxx {} not-fetched {} not-read {} largest {} work {} memory {} memory-per-kib {} syntax-depth {} facts {}",
                p.candidates,
                p.files,
                p.not_cxx,
                p.not_fetched,
                p.not_read,
                p.largest,
                p.work,
                p.memory,
                p.memory_per_kib,
                p.syntax_depth,
                p.facts,
            );
            println!(
                "over-default file-size {} work {} memory {} syntax-depth {} entry-facts {}",
                p.over_file_size,
                p.over_work,
                p.over_memory,
                p.over_syntax_depth,
                p.over_entry_facts,
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("boxt: in-process measurement failed: {e}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn bench(root: &std::path::Path, runs: u32, jobs: Option<NonZeroU32>) -> ExitCode {
    let walked = match cxx_jobs(root) {
        Ok(walked) => walked,
        Err(e) => {
            eprintln!("boxt: cannot walk {}: {e}", root.display());
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let files = walked.jobs;
    if files.is_empty() {
        eprintln!("boxt: no C/C++ file under {}", root.display());
        return ExitCode::from(EXIT_ERROR);
    }
    let config = match PoolConfig::new(pool::default_settings(), jobs) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("boxt: cannot find its own executable: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    let threads = config.jobs;
    println!(
        "tree {} files {} skipped {} workers {threads} threads {threads} runs {runs}",
        root.display(),
        files.len(),
        walked.skipped,
    );
    if let Err(e) = parse_in_process(&files, threads) {
        eprintln!("boxt: one-process warm-up failed: {e}");
        return ExitCode::from(EXIT_ERROR);
    }
    if let Err(e) = pool::run(&config, files.clone()) {
        eprintln!("boxt: {e}");
        return ExitCode::from(EXIT_ERROR);
    }
    let mut single = Vec::new();
    let mut pooled = Vec::new();
    for run in 1..=runs {
        let pool_first = run % 2 == 0;
        let mut one = None;
        let mut pool = None;
        for pool_turn in [pool_first, !pool_first] {
            let start = Instant::now();
            if pool_turn {
                match pool::run(&config, files.clone()) {
                    Ok(report) => {
                        pool = Some((start.elapsed(), report.read_count(), report.spawns))
                    }
                    Err(e) => {
                        eprintln!("boxt: {e}");
                        return ExitCode::from(EXIT_ERROR);
                    }
                }
            } else {
                match parse_in_process(&files, threads) {
                    Ok(read) => one = Some((start.elapsed(), read)),
                    Err(e) => {
                        eprintln!("boxt: one-process run failed: {e}");
                        return ExitCode::from(EXIT_ERROR);
                    }
                }
            }
        }
        let (Some((one, read)), Some((pool, pool_read, spawns))) = (one, pool) else {
            unreachable!("both modes ran");
        };
        println!(
            "run {run} ({} first) one-process {:.3}s ({read} read) pool {:.3}s ({pool_read} read, {spawns} spawns)",
            if pool_first { "pool" } else { "one-process" },
            one.as_secs_f64(),
            pool.as_secs_f64(),
        );
        if read != pool_read {
            eprintln!("boxt: the pool read {pool_read} files and one process {read}");
            return ExitCode::from(EXIT_ERROR);
        }
        single.push(one);
        pooled.push(pool);
    }
    let one = median(&mut single);
    let pool = median(&mut pooled);
    let overhead = (pool.as_secs_f64() / one.as_secs_f64() - 1.0) * 100.0;
    let pass = overhead <= 20.0;
    println!(
        "median one-process {:.3}s pool {:.3}s overhead {overhead:+.1}% {}",
        one.as_secs_f64(),
        pool.as_secs_f64(),
        if pass { "pass" } else { "miss" }
    );
    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_ERROR)
    }
}

fn median(times: &mut [Duration]) -> Duration {
    times.sort();
    let mid = times.len() / 2;
    if times.len().is_multiple_of(2) {
        (times[mid - 1] + times[mid]) / 2
    } else {
        times[mid]
    }
}
