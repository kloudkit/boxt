use std::collections::BTreeMap;

use boxt_core::SortedVec;
use boxt_core::verdict::{
    BehaviourId, Confidence, Group, GroupKey, ReportedFinding, RuleId, RuleSection,
};

pub(crate) fn by_behaviour(findings: &SortedVec<ReportedFinding>) -> Vec<Group> {
    let mut groups: BTreeMap<&BehaviourId, Group> = BTreeMap::new();
    let mut sections: BTreeMap<(&BehaviourId, &RuleId), RuleSection> = BTreeMap::new();
    for reported in findings.iter() {
        let f = &reported.finding;
        let group = groups.entry(&f.behaviour).or_insert_with(|| Group {
            key: GroupKey::Behaviour(f.behaviour.clone()),
            found: 0,
            suspected: 0,
            rules: Vec::new(),
        });
        let count = match f.confidence {
            Confidence::Found => &mut group.found,
            Confidence::Suspected => &mut group.suspected,
        };
        *count = count.saturating_add(1);
        sections
            .entry((&f.behaviour, &f.rule))
            .or_insert_with(|| RuleSection {
                rule: f.rule.clone(),
                fix: f.fix.clone(),
                findings: Vec::new(),
            })
            .findings
            .push(reported.id.clone());
    }
    for ((behaviour, _), section) in sections {
        if let Some(group) = groups.get_mut(behaviour) {
            group.rules.push(section);
        }
    }
    groups.into_values().collect()
}
