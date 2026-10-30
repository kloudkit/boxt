use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use boxt_collector::CapAllocator;
use boxt_collector::bench::{cxx_jobs, parse_in_process};
use boxt_collector::pool::{self, PoolConfig};
use boxt_collector::worker::WORKER_SUBCOMMAND;
use clap::{Parser, Subcommand};

#[global_allocator]
static ALLOCATOR: CapAllocator = CapAllocator;

const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(name = "boxt", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(name = WORKER_SUBCOMMAND, hide = true)]
    Worker,
    #[command(hide = true)]
    Bench {
        root: PathBuf,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..))]
        runs: u32,
        #[arg(long)]
        jobs: Option<NonZeroU32>,
    },
}

fn main() -> ExitCode {
    boxt_collector::os::suppress_error_dialogs();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Worker) => boxt_collector::worker::run(),
        Some(Command::Bench { root, runs, jobs }) => bench(&root, runs, jobs),
        None => ExitCode::SUCCESS,
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
