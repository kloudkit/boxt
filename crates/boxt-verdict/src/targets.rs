use boxt_core::SortedVec;
use boxt_core::verdict::{
    BehaviourId, Confidence, Finding, Profile, TargetId, TargetVerdict, Tier, TierRange,
};

use crate::VerdictError;

pub(crate) fn resolve<'a>(
    named: &SortedVec<TargetId>,
    profiles: &'a [Profile],
) -> Result<Vec<&'a Profile>, VerdictError> {
    if named.is_empty() {
        return Ok(profiles.iter().collect());
    }
    let mut chosen = Vec::with_capacity(named.len());
    for id in named.iter() {
        if chosen.iter().any(|p: &&Profile| &p.id == id) {
            continue;
        }
        match profiles.iter().find(|p| &p.id == id) {
            Some(profile) => chosen.push(profile),
            None => {
                return Err(VerdictError::UnknownTarget {
                    id: id.clone(),
                    known: profiles.iter().map(|p| p.id.clone()).collect(),
                });
            }
        }
    }
    Ok(chosen)
}

pub(crate) fn tier(profile: &Profile, behaviour: &BehaviourId) -> Option<Tier> {
    let mapped = profile
        .controls
        .iter()
        .flat_map(|c| c.behaviours.iter())
        .filter(|b| &b.behaviour == behaviour)
        .map(|b| b.tier)
        .min();
    let allowed = profile.allowed.contains(behaviour).then_some(Tier::Ready);
    match (mapped, allowed) {
        (Some(m), Some(a)) => Some(m.worst(a)),
        (m, a) => m.or(a),
    }
}

pub(crate) fn verdict(profile: &Profile, findings: &SortedVec<Finding>) -> TargetVerdict {
    let mut found_tier = Tier::Ready;
    let mut with_suspected = Tier::Ready;
    let mut found = 0u64;
    let mut suspected = 0u64;
    let mut allowed = 0u64;
    let mut unmapped = Vec::new();
    for finding in findings.iter() {
        let Some(t) = tier(profile, &finding.behaviour) else {
            if !unmapped.contains(&finding.behaviour) {
                unmapped.push(finding.behaviour.clone());
            }
            continue;
        };
        with_suspected = with_suspected.worst(t);
        if finding.confidence == Confidence::Found {
            found_tier = found_tier.worst(t);
        }
        let count = match (t, finding.confidence) {
            (Tier::Ready, _) => &mut allowed,
            (_, Confidence::Found) => &mut found,
            (_, Confidence::Suspected) => &mut suspected,
        };
        *count = count.saturating_add(1);
    }
    TargetVerdict {
        target: profile.id.clone(),
        range: TierRange {
            found: found_tier,
            with_suspected,
        },
        found,
        suspected,
        allowed,
        unmapped: unmapped.into(),
    }
}
