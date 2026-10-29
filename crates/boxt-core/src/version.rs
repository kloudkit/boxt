use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SchemaVersion {
    pub major: u32,
    pub minor: u32,
}

impl SchemaVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        SchemaVersion { major, minor }
    }

    pub const fn accepts(self, found: SchemaVersion) -> bool {
        found.major == self.major && found.minor <= self.minor
    }

    pub fn check(self, found: SchemaVersion) -> Result<(), UnsupportedVersion> {
        let reason = if found.major != self.major {
            UnsupportedReason::OtherMajor
        } else if found.minor > self.minor {
            UnsupportedReason::NewerMinor
        } else {
            return Ok(());
        };
        Err(UnsupportedVersion {
            supported: self,
            found,
            reason,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedReason {
    OtherMajor,
    NewerMinor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedVersion {
    pub supported: SchemaVersion,
    pub found: SchemaVersion,
    pub reason: UnsupportedReason,
}

impl fmt::Display for UnsupportedVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            UnsupportedReason::OtherMajor => write!(
                f,
                "version {} is not supported; this build reads {}.x up to {}",
                self.found, self.supported.major, self.supported
            ),
            UnsupportedReason::NewerMinor => write!(
                f,
                "version {} is newer than {}, the newest this build reads",
                self.found, self.supported
            ),
        }
    }
}

impl std::error::Error for UnsupportedVersion {}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaVersionError;

impl fmt::Display for SchemaVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a schema version is <major>.<minor>, both decimal numbers")
    }
}

impl std::error::Error for SchemaVersionError {}

fn part(s: &str) -> Result<u32, SchemaVersionError> {
    let canonical = s == "0" || (!s.is_empty() && !s.starts_with('0'));
    if !canonical || !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(SchemaVersionError);
    }
    s.parse().map_err(|_| SchemaVersionError)
}

impl FromStr for SchemaVersion {
    type Err = SchemaVersionError;

    fn from_str(s: &str) -> Result<Self, SchemaVersionError> {
        let (major, minor) = s.split_once('.').ok_or(SchemaVersionError)?;
        Ok(SchemaVersion::new(part(major)?, part(minor)?))
    }
}

impl TryFrom<String> for SchemaVersion {
    type Error = SchemaVersionError;

    fn try_from(s: String) -> Result<Self, SchemaVersionError> {
        s.parse()
    }
}

impl From<SchemaVersion> for String {
    fn from(v: SchemaVersion) -> String {
        v.to_string()
    }
}

pub const RULE_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new(1, 0);
pub const PROFILE_FORMAT_VERSION: SchemaVersion = SchemaVersion::new(1, 0);
pub const WAIVER_FILE_VERSION: SchemaVersion = SchemaVersion::new(1, 0);
pub const PROJECT_FILE_VERSION: SchemaVersion = SchemaVersion::new(1, 0);
pub const JSON_OUTPUT_VERSION: SchemaVersion = SchemaVersion::new(1, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Versions {
    pub rule_schema: SchemaVersion,
    pub profile_format: SchemaVersion,
    pub waiver_file: SchemaVersion,
    pub project_file: SchemaVersion,
    pub json_output: SchemaVersion,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_print_and_parse_as_major_dot_minor() {
        assert_eq!(RULE_SCHEMA_VERSION.to_string(), "1.0");
        assert_eq!("2.13".parse(), Ok(SchemaVersion::new(2, 13)));
        assert_eq!("0.0".parse(), Ok(SchemaVersion::new(0, 0)));
        for bad in [
            "1", "1.", ".1", "1.0.0", "01.0", "1.01", "+1.0", "1.-1", "a.b", "",
        ] {
            assert_eq!(
                bad.parse::<SchemaVersion>(),
                Err(SchemaVersionError),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_loader_accepts_its_own_major_up_to_its_own_minor() {
        let loader = SchemaVersion::new(1, 2);
        assert!(loader.accepts(SchemaVersion::new(1, 0)));
        assert!(loader.accepts(SchemaVersion::new(1, 2)));
        assert!(!loader.accepts(SchemaVersion::new(1, 3)));
        assert!(!loader.accepts(SchemaVersion::new(0, 2)));
        assert!(!loader.accepts(SchemaVersion::new(2, 0)));
        assert_eq!(loader.check(SchemaVersion::new(1, 1)), Ok(()));
        let newer = loader.check(SchemaVersion::new(1, 9)).unwrap_err();
        assert_eq!(
            newer,
            UnsupportedVersion {
                supported: loader,
                found: SchemaVersion::new(1, 9),
                reason: UnsupportedReason::NewerMinor,
            }
        );
        assert_eq!(
            newer.to_string(),
            "version 1.9 is newer than 1.2, the newest this build reads"
        );
        assert_eq!(
            loader.check(SchemaVersion::new(2, 0)).unwrap_err().reason,
            UnsupportedReason::OtherMajor
        );
        assert!(SchemaVersion::new(1, 9) < SchemaVersion::new(2, 0));
        assert!(SchemaVersion::new(1, 2) < SchemaVersion::new(1, 10));
    }

    #[test]
    fn versions_serialize_as_major_dot_minor_strings() {
        let v = SchemaVersion::new(1, 10);
        assert_eq!(serde_json::to_string(&v).unwrap(), r#""1.10""#);
        assert_eq!(
            serde_json::from_str::<SchemaVersion>(r#""1.10""#).unwrap(),
            v
        );
        assert!(serde_json::from_str::<SchemaVersion>(r#""1""#).is_err());
        assert!(serde_json::from_str::<SchemaVersion>(r#"{"major":1,"minor":0}"#).is_err());
    }
}
