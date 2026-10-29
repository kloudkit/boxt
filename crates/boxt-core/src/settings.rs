use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::limits::Limits;
use crate::sorted::SortedVec;
use crate::text::Text;
use crate::verdict::TargetId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PathPattern {
    pub base: Text,
    pub pattern: Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Latin1,
    WindowsCodePage(u16),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EncodingDecl {
    pub pattern: PathPattern,
    pub encoding: Encoding,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerSettings {
    pub limits: Limits,
    pub encodings: Vec<EncodingDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Settings {
    pub inputs: Vec<Text>,
    pub targets: SortedVec<TargetId>,
    pub limits: Limits,
    pub encodings: Vec<EncodingDecl>,
    pub excludes: Vec<PathPattern>,
    pub ignored_macros: SortedVec<Text>,
    pub jobs: Option<NonZeroU32>,
}

impl Settings {
    pub fn worker(&self) -> WorkerSettings {
        WorkerSettings {
            limits: self.limits.clone(),
            encodings: self.encodings.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::LimitName;

    #[test]
    fn v4_worker_settings_carry_limits_and_encodings_only() {
        let encodings = vec![EncodingDecl {
            pattern: PathPattern {
                base: Text::from("/repo"),
                pattern: Text::from("legacy/**"),
            },
            encoding: Encoding::Latin1,
        }];
        let settings = Settings {
            inputs: vec![Text::from("/repo")],
            targets: vec![TargetId::new("docker").unwrap()].into(),
            limits: Limits::default().with(LimitName::Work, 7).unwrap(),
            encodings: encodings.clone(),
            excludes: vec![PathPattern {
                base: Text::from("/repo"),
                pattern: Text::from("vendor/**"),
            }],
            ignored_macros: vec![Text::from("DEBUG")].into(),
            jobs: NonZeroU32::new(2),
        };
        let worker = settings.worker();
        assert_eq!(worker.limits, settings.limits);
        assert_eq!(worker.limits.get(LimitName::Work), 7);
        assert_eq!(worker.encodings, encodings);
    }

    #[test]
    fn zero_jobs_is_refused() {
        let json = r#"{"inputs":[],"targets":[],"limits":LIMITS,"encodings":[],"excludes":[],"ignored_macros":[],"jobs":0}"#
            .replace("LIMITS", &serde_json::to_string(&Limits::default()).unwrap());
        assert!(serde_json::from_str::<Settings>(&json).is_err());
        let ok = json.replace(r#""jobs":0"#, r#""jobs":4"#);
        assert_eq!(
            serde_json::from_str::<Settings>(&ok).unwrap().jobs,
            NonZeroU32::new(4)
        );
    }
}
