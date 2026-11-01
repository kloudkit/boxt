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
use boxt_core::outcome::{FileOutcome, sm3_share, verdict_counts};
use boxt_core::verdict::{Finding, Observation, Profile, ProjectFile, Result, TargetId};
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
