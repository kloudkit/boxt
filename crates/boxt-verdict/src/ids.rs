use std::collections::BTreeMap;
use std::num::NonZeroU32;

use boxt_core::identity::{FindingId, Identity};
use boxt_core::verdict::{Finding, ReportedFinding};
use boxt_core::{SortedVec, Text};
use sha2::{Digest, Sha256};

pub(crate) fn assign(findings: &SortedVec<Finding>, roots: &[Text]) -> SortedVec<ReportedFinding> {
    let relative = relative_roots(roots);
    let mut groups: BTreeMap<Identity, Vec<usize>> = BTreeMap::new();
    for (i, finding) in findings.iter().enumerate() {
        groups.entry(Identity::of(finding)).or_default().push(i);
    }
    let root_of = |f: &Finding| -> &[u8] {
        usize::try_from(f.location.input.index)
            .ok()
            .and_then(|i| relative.get(i))
            .map_or(&[][..], Vec::as_slice)
    };
    let mut ids: Vec<Option<FindingId>> = vec![None; findings.len()];
    for (identity, mut members) in groups {
        members.sort_by(|&a, &b| {
            let (fa, fb) = (&findings[a], &findings[b]);
            root_of(fa)
                .cmp(root_of(fb))
                .then_with(|| place(fa).cmp(&place(fb)))
                .then_with(|| fa.rule.cmp(&fb.rule))
                .then_with(|| fa.rule.is_user().cmp(&fb.rule.is_user()))
                .then_with(|| a.cmp(&b))
        });
        let digest: [u8; 32] = Sha256::digest(identity.canonical_text().as_bytes()).into();
        for (n, i) in members.into_iter().enumerate() {
            let ordinal = u32::try_from(n + 1)
                .ok()
                .and_then(NonZeroU32::new)
                .unwrap_or(NonZeroU32::MAX);
            ids[i] = Some(FindingId::new(&digest, ordinal));
        }
    }
    findings
        .iter()
        .zip(ids)
        .filter_map(|(finding, id)| {
            id.map(|id| ReportedFinding {
                finding: finding.clone(),
                id,
            })
        })
        .collect()
}

fn place(f: &Finding) -> impl Ord + '_ {
    let l = &f.location;
    (&l.frames, &l.path, l.line, &l.scope)
}

fn components(root: &Text) -> Vec<&[u8]> {
    root.as_bytes()
        .split(|&b| is_separator(b))
        .filter(|c| !c.is_empty())
        .collect()
}

fn is_separator(b: u8) -> bool {
    b == b'/' || (cfg!(windows) && b == b'\\')
}

pub(crate) fn relative_roots(roots: &[Text]) -> Vec<Vec<u8>> {
    let split: Vec<Vec<&[u8]>> = roots.iter().map(components).collect();
    let common = split.first().map_or(0, |first| {
        let mut n = first.len();
        for other in &split[1..] {
            n = n.min(first.iter().zip(other).take_while(|(a, b)| a == b).count());
        }
        n
    });
    split
        .iter()
        .map(|parts| parts[common.min(parts.len())..].join(&b'/'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(roots: &[&str]) -> Vec<String> {
        let roots: Vec<Text> = roots.iter().map(|r| Text::from(*r)).collect();
        relative_roots(&roots)
            .into_iter()
            .map(|r| String::from_utf8(r).unwrap())
            .collect()
    }

    #[test]
    fn roots_are_taken_relative_to_their_common_ancestor() {
        assert_eq!(rel(&["/a/x", "/b/x"]), ["a/x", "b/x"]);
        assert_eq!(rel(&["/b/x", "/a/x"]), ["b/x", "a/x"]);
        assert_eq!(rel(&["/w/a/x/", "/w/a/y"]), ["x", "y"]);
        assert_eq!(rel(&["/w/a", "/w/a/b"]), ["", "b"]);
        assert_eq!(rel(&["/only/one"]), [""]);
        assert_eq!(rel(&["/s/p", "/s/p"]), ["", ""]);
        assert!(rel(&[]).is_empty());
    }
}
