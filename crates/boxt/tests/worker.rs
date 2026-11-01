use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};

use boxt_collector::bench::cxx_jobs;
use boxt_collector::pool::{self, PoolConfig};

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree(name: &str, files: &[&str]) -> Dir {
    let root = std::env::temp_dir().join(format!("boxt-bin-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (i, file) in files.iter().enumerate() {
        let body = format!("int f{i}(int x) {{\n  return x + {i};\n}}\n");
        std::fs::write(root.join(file), body).unwrap();
    }
    Dir(root)
}

fn binary_config(settings_cap: Option<u64>) -> PoolConfig {
    let settings = match settings_cap {
        Some(cap) => pool::settings_with_memory_cap(cap).unwrap(),
        None => pool::default_settings(),
    };
    let mut config = PoolConfig::new(settings, NonZeroU32::new(1)).unwrap();
    config.program = Path::new(env!("CARGO_BIN_EXE_boxt")).to_path_buf();
    config.jobs = NonZeroUsize::MIN;
    config
}

#[test]
fn the_boxt_binary_reads_ordinary_files_as_a_worker() {
    let dir = tree("ordinary", &["a.c", "b.cpp", "c.h"]);
    let jobs = cxx_jobs(&dir.0).unwrap().jobs;
    let report = pool::run(&binary_config(None), jobs).unwrap();
    assert_eq!(report.jobs.len(), 3);
    assert_eq!(report.read_count(), 3);
    assert_eq!(report.spawns, 1);
}

#[cfg(feature = "fault-fixtures")]
#[test]
fn the_boxt_binary_exits_on_its_memory_cap() {
    let dir = tree("alloc", &["a.c", "boxt-fixture-alloc.c", "z.c"]);
    let jobs = cxx_jobs(&dir.0).unwrap().jobs;
    let report = pool::run(&binary_config(Some(256 * 1024 * 1024)), jobs).unwrap();
    assert_eq!(report.read_count(), 2);
    let fixture = report
        .jobs
        .iter()
        .find(|r| format!("{:?}", r.job.file).contains("boxt-fixture-alloc"))
        .unwrap();
    assert!(
        format!("{:?}", fixture.outcome.file).contains("LimitExceeded(WorkerMemoryCap)"),
        "{:?}",
        fixture.outcome
    );
    assert!(fixture.outcome.host_dependent);
    assert_eq!(fixture.attempts, 2);
}

fn collect(args: &[&std::ffi::OsStr]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_boxt"))
        .arg("collect")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn collect_prints_counts_and_exits_2_when_nothing_is_read() {
    let dir = tree("collect", &["a.c", "b.cpp"]);
    std::fs::write(dir.0.join("README"), b"text\n").unwrap();
    let out = collect(&[dir.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("outcomes 3 read 2 not-checked 1 "),
        "{text}"
    );
    let one = collect(&[dir.0.as_os_str(), "--jobs".as_ref(), "1".as_ref()]);
    assert_eq!(String::from_utf8(one.stdout).unwrap(), text);

    let empty = tree("collect-nothing", &[]);
    std::fs::write(empty.0.join("README"), b"text\n").unwrap();
    let out = collect(&[empty.0.as_os_str()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let entries: Vec<_> = std::fs::read_dir(&empty.0).unwrap().collect();
    assert_eq!(entries.len(), 1, "nothing was written into the input");
}
