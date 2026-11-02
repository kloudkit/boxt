//! Targets, tiers, finding identity, waivers, the answer sheet, the gate and groups.

#![forbid(unsafe_code)]

mod groups;
mod ids;
mod not_checked;
mod scope;
mod targets;

pub use boxt_core;

use std::fmt;

use boxt_core::message::CollectedFile;
use boxt_core::outcome::{FileOutcome, OutOfScopeCause, sm3_share, verdict_counts};
use boxt_core::settings::PathPattern;
use boxt_core::verdict::{Excluded, Finding, Observation, Profile, ProjectFile, Result, TargetId};
use boxt_core::version::Versions;
use boxt_core::{SortedVec, Text, input_scope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct VerdictInput<'a> {
    pub files: &'a SortedVec<CollectedFile>,
    pub findings: &'a SortedVec<Finding>,
    pub observations: &'a SortedVec<Observation>,
    pub macro_tables_cut: &'a SortedVec<u32>,
    pub host_dependent_trees: &'a SortedVec<u32>,
    pub roots: &'a [Text],
    pub compile_database_used: bool,
    pub project_file: &'a ProjectFile,
    pub excludes: &'a [PathPattern],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared<'a> {
    profiles: Vec<&'a Profile>,
}

impl<'a> Prepared<'a> {
    pub fn profiles(&self) -> &[&'a Profile] {
        &self.profiles
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictError {
    UnknownTarget {
        id: TargetId,
        known: SortedVec<TargetId>,
    },
    NothingRead,
}

impl fmt::Display for VerdictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerdictError::UnknownTarget { id, known } => {
                write!(f, "unknown target {id}; known targets:")?;
                for k in known.iter() {
                    write!(f, " {k}")?;
                }
                Ok(())
            }
            VerdictError::NothingRead => f.write_str("no file of any kind was read"),
        }
    }
}

impl std::error::Error for VerdictError {}

pub fn prepare<'a>(
    profiles: &'a [Profile],
    targets: &SortedVec<TargetId>,
) -> std::result::Result<Prepared<'a>, VerdictError> {
    Ok(Prepared {
        profiles: targets::resolve(targets, profiles)?,
    })
}

fn excluded(files: &SortedVec<CollectedFile>, excludes: &[PathPattern]) -> Excluded {
    let mut patterns: Vec<(PathPattern, u64)> = excludes.iter().map(|p| (p.clone(), 0)).collect();
    let mut own_files = Vec::new();
    for file in files.iter() {
        let FileOutcome::OutOfScope { cause } = &file.outcome.file else {
            continue;
        };
        match cause {
            OutOfScopeCause::Pattern(pattern) => {
                if let Some((_, count)) = patterns.iter_mut().find(|(p, _)| p == pattern) {
                    *count = count.saturating_add(1);
                }
            }
            OutOfScopeCause::OwnFile(role) => own_files.push((file.location.clone(), *role)),
        }
    }
    Excluded {
        patterns,
        own_files,
    }
}

pub fn build(
    prepared: &Prepared<'_>,
    input: &VerdictInput<'_>,
) -> std::result::Result<Result, VerdictError> {
    if !input
        .files
        .iter()
        .any(|f| matches!(f.outcome.file, FileOutcome::Read))
    {
        return Err(VerdictError::NothingRead);
    }
    let input_scope = scope::of(input.files);
    let host_dependent = !input.host_dependent_trees.is_empty()
        || input.files.iter().any(|f| f.outcome.host_dependent);
    let findings = ids::assign(input.findings, input.roots);
    let not_checked = not_checked::entries(input.files, input.observations);
    Ok(Result {
        versions: Versions::CURRENT,
        project_file: input.project_file.clone(),
        excluded: excluded(input.files, input.excludes),
        input_scope,
        input_scope_text: input_scope::text(input_scope, input.compile_database_used),
        counts: verdict_counts(input.files.iter().map(|f| &f.outcome.file)),
        sm3: sm3_share(input.files.iter()),
        macro_tables_cut: input.macro_tables_cut.clone(),
        host_dependent,
        targets: prepared
            .profiles
            .iter()
            .map(|p| targets::verdict(p, input.findings))
            .collect(),
        groups: groups::by_behaviour(&findings),
        no_reader: not_checked::no_reader(&not_checked),
        findings,
        not_checked,
        observations: input.observations.clone(),
    })
}
