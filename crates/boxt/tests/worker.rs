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
