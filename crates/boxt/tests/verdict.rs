mod common;

use std::fs;
use std::path::{Path, PathBuf};

use boxt_collector::boxt_core::input_scope::{self, InputScope};
use boxt_collector::boxt_core::verdict::{Result, Tier, TierRange};

use common::{seed_dir, verdict};

fn copy_seed(to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(seed_dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn the_seed_run_has_a_verdict_for_every_target() {
    let result = verdict(&[seed_dir()]);
    let targets: Vec<&str> = result.targets.iter().map(|t| t.target.as_str()).collect();
    assert_eq!(
        targets,
        [
            "docker-default",
            "kubernetes-restricted",
            "openshift-restricted-v2",
            "rootless-podman"
        ]
    );
    assert_eq!(result.input_scope, InputScope::SourceOnly);
    assert_eq!(
        result.input_scope_text,
        input_scope::text(InputScope::SourceOnly, false)
    );
    assert_eq!(result.findings.len(), 13);
    for reported in result.findings.iter() {
        assert_eq!(reported.id.as_str().len(), 14, "{}", reported.id);
    }
    let kubernetes = &result.targets[1];
    assert_eq!(
        kubernetes.range,
        TierRange {
            found: Tier::Blocked,
            with_suspected: Tier::Blocked
        }
    );
    assert_eq!(
        (kubernetes.found, kubernetes.suspected, kubernetes.allowed),
        (12, 1, 0)
    );
    let docker = &result.targets[0];
    assert_eq!(docker.range.found, Tier::NeedsWindows);
    assert_eq!((docker.found, docker.suspected, docker.allowed), (3, 0, 10));
    assert!(result.targets.iter().all(|t| t.unmapped.is_empty()));
    assert_eq!(result.counts.read, 5);
    assert!(!result.host_dependent);
}

#[test]
fn the_three_setuid_calls_in_one_function_share_an_identity() {
    let result = verdict(&[seed_dir()]);
    let ids: Vec<(Option<u32>, String)> = result
        .findings
        .iter()
        .filter(|r| {
            r.finding.behaviour.as_str() == "setuid"
                && r.finding.location.path.as_ref().unwrap().as_bytes() == b"setuid.c"
        })
        .map(|r| (r.finding.location.line, r.id.to_string()))
        .collect();
    assert_eq!(ids.len(), 3);
    let digest = &ids[0].1[..12];
    for (n, (_, id)) in ids.iter().enumerate() {
        assert_eq!(&id[..12], digest);
        assert_eq!(id[13..], (n + 1).to_string());
    }
    assert_eq!(
        ids.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
        [Some(7), Some(9), Some(11)]
    );
}

fn ids_by_root(
    result: &Result,
    roots: &[PathBuf],
) -> Vec<(String, String, Option<u32>, String, String)> {
    let mut out: Vec<_> = result
        .findings
        .iter()
        .map(|r| {
            let index = r.finding.location.input.index as usize;
            let root = roots[index]
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (
                root,
                utf8(r.finding.location.path.as_ref().unwrap().as_bytes()),
                r.finding.location.line,
                r.finding.rule.to_string(),
                r.id.to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn ids_are_the_same_whatever_the_input_order() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("verdict-two-roots");
    let _ = fs::remove_dir_all(&base);
    let p = base.join("a").join("x");
    let q = base.join("b").join("x");
    copy_seed(&p);
    copy_seed(&q);
    let pq = [p.clone(), q.clone()];
    let qp = [q, p];
    let first = ids_by_root(&verdict(&pq), &pq);
    let second = ids_by_root(&verdict(&qp), &qp);
    assert_eq!(first.len(), 26);
    assert_eq!(first, second);
    let a = first.iter().find(|f| f.0 == "a").unwrap();
    let b = first
        .iter()
        .find(|f| f.0 == "b" && f.1 == a.1 && f.2 == a.2 && f.3 == a.3)
        .unwrap();
    assert_eq!(a.4[..12], b.4[..12]);
    assert_ne!(a.4, b.4);
}
