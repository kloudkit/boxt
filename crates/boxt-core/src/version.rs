use serde::{Deserialize, Serialize};

pub const RULE_SCHEMA_VERSION: u32 = 1;
pub const PROFILE_FORMAT_VERSION: u32 = 1;
pub const WAIVER_FILE_VERSION: u32 = 1;
pub const PROJECT_FILE_VERSION: u32 = 1;
pub const JSON_OUTPUT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Versions {
    pub rule_schema: u32,
    pub profile_format: u32,
    pub waiver_file: u32,
    pub project_file: u32,
    pub json_output: u32,
}

impl Versions {
    pub const CURRENT: Versions = Versions {
        rule_schema: RULE_SCHEMA_VERSION,
        profile_format: PROFILE_FORMAT_VERSION,
        waiver_file: WAIVER_FILE_VERSION,
        project_file: PROJECT_FILE_VERSION,
        json_output: JSON_OUTPUT_VERSION,
    };
}
