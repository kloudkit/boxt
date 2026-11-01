use std::fmt;

use serde::{Deserialize, Serialize};

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum LimitName {
        FileSize,
        Work,
        Memory,
        ArchiveDepth,
        DecompressionRatio,
        TotalEntries,
        TotalDecompressedBytes,
        Watchdog,
        RunWatchdog,
        WorkerMemoryCap,
        EntryFacts,
        EntryMessageBytes,
        MacroTableBytes,
        SyntaxDepth,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitScope {
    PerFile,
    RunWide,
    PerEntry,
    LastResort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitUnit {
    Bytes,
    Steps,
    Levels,
    Ratio,
    Entries,
    Facts,
    Seconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitSpec {
    pub name: LimitName,
    pub flag: &'static str,
    pub scope: LimitScope,
    pub unit: LimitUnit,
    pub default: u64,
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

pub const LIMITS: &[LimitSpec] = &[
    LimitSpec {
        name: LimitName::FileSize,
        flag: "file-size",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Bytes,
        default: 16 * MIB,
    },
    LimitSpec {
        name: LimitName::Work,
        flag: "work",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Steps,
        default: 2_000_000,
    },
    LimitSpec {
        name: LimitName::Memory,
        flag: "memory",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Bytes,
        default: 512 * MIB,
    },
    LimitSpec {
        name: LimitName::ArchiveDepth,
        flag: "archive-depth",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Levels,
        default: 8,
    },
    LimitSpec {
        name: LimitName::DecompressionRatio,
        flag: "decompression-ratio",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Ratio,
        default: 100,
    },
    LimitSpec {
        name: LimitName::TotalEntries,
        flag: "total-entries",
        scope: LimitScope::RunWide,
        unit: LimitUnit::Entries,
        default: 100_000,
    },
    LimitSpec {
        name: LimitName::TotalDecompressedBytes,
        flag: "total-decompressed-bytes",
        scope: LimitScope::RunWide,
        unit: LimitUnit::Bytes,
        default: 16 * GIB,
    },
    LimitSpec {
        name: LimitName::Watchdog,
        flag: "watchdog",
        scope: LimitScope::LastResort,
        unit: LimitUnit::Seconds,
        default: 600,
    },
    LimitSpec {
        name: LimitName::RunWatchdog,
        flag: "run-watchdog",
        scope: LimitScope::LastResort,
        unit: LimitUnit::Seconds,
        default: 6 * 60 * 60,
    },
    LimitSpec {
        name: LimitName::WorkerMemoryCap,
        flag: "worker-memory-cap",
        scope: LimitScope::LastResort,
        unit: LimitUnit::Bytes,
        default: 2 * GIB,
    },
    LimitSpec {
        name: LimitName::EntryFacts,
        flag: "entry-facts",
        scope: LimitScope::PerEntry,
        unit: LimitUnit::Facts,
        default: 50_000,
    },
    LimitSpec {
        name: LimitName::EntryMessageBytes,
        flag: "entry-message-bytes",
        scope: LimitScope::PerEntry,
        unit: LimitUnit::Bytes,
        default: 64 * MIB,
    },
    LimitSpec {
        name: LimitName::MacroTableBytes,
        flag: "macro-table-bytes",
        scope: LimitScope::RunWide,
        unit: LimitUnit::Bytes,
        default: 8 * MIB,
    },
    LimitSpec {
        name: LimitName::SyntaxDepth,
        flag: "syntax-depth",
        scope: LimitScope::PerFile,
        unit: LimitUnit::Levels,
        default: 10_000,
    },
];

impl LimitName {
    pub fn spec(self) -> &'static LimitSpec {
        &LIMITS[self as usize]
    }

    pub fn flag(self) -> &'static str {
        self.spec().flag
    }

    pub fn default_value(self) -> u64 {
        self.spec().default
    }

    pub fn from_flag(flag: &str) -> Option<LimitName> {
        LIMITS.iter().find(|s| s.flag == flag).map(|s| s.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    Zero(LimitName),
    AboveMax { name: LimitName, max: u64 },
    CapBelowMemory { cap: u64, memory: u64 },
    TableAboveCap { table: u64, cap: u64 },
}

pub const MEMORY_CAP_FACTOR: u64 = 4;

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimitError::Zero(name) => write!(f, "limit {} must be at least 1", name.flag()),
            LimitError::AboveMax { name, max } => {
                write!(f, "limit {} must be at most {max}", name.flag())
            }
            LimitError::CapBelowMemory { cap, memory } => write!(
                f,
                "limit {} ({cap}) must be at least {MEMORY_CAP_FACTOR} times limit {} ({memory})",
                LimitName::WorkerMemoryCap.flag(),
                LimitName::Memory.flag()
            ),
            LimitError::TableAboveCap { table, cap } => write!(
                f,
                "limit {} ({table}) must be at most limit {} ({cap})",
                LimitName::MacroTableBytes.flag(),
                LimitName::WorkerMemoryCap.flag()
            ),
        }
    }
}

impl std::error::Error for LimitError {}

fn check(name: LimitName, value: u64) -> Result<(), LimitError> {
    if value == 0 {
        return Err(LimitError::Zero(name));
    }
    let max = match name {
        LimitName::EntryMessageBytes => u64::from(u32::MAX),
        _ => u64::MAX,
    };
    if value > max {
        return Err(LimitError::AboveMax { name, max });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitValues {
    file_size: u64,
    work: u64,
    memory: u64,
    archive_depth: u64,
    decompression_ratio: u64,
    total_entries: u64,
    total_decompressed_bytes: u64,
    watchdog: u64,
    run_watchdog: u64,
    worker_memory_cap: u64,
    entry_facts: u64,
    entry_message_bytes: u64,
    macro_table_bytes: u64,
    syntax_depth: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "LimitValues", into = "LimitValues")]
pub struct Limits(LimitValues);

impl Limits {
    pub fn get(&self, name: LimitName) -> u64 {
        let v = &self.0;
        match name {
            LimitName::FileSize => v.file_size,
            LimitName::Work => v.work,
            LimitName::Memory => v.memory,
            LimitName::ArchiveDepth => v.archive_depth,
            LimitName::DecompressionRatio => v.decompression_ratio,
            LimitName::TotalEntries => v.total_entries,
            LimitName::TotalDecompressedBytes => v.total_decompressed_bytes,
            LimitName::Watchdog => v.watchdog,
            LimitName::RunWatchdog => v.run_watchdog,
            LimitName::WorkerMemoryCap => v.worker_memory_cap,
            LimitName::EntryFacts => v.entry_facts,
            LimitName::EntryMessageBytes => v.entry_message_bytes,
            LimitName::MacroTableBytes => v.macro_table_bytes,
            LimitName::SyntaxDepth => v.syntax_depth,
        }
    }

    pub fn set(&mut self, name: LimitName, value: u64) -> Result<(), LimitError> {
        check(name, value)?;
        self.put(name, value);
        Ok(())
    }

    pub fn validate(&self) -> Result<(), LimitError> {
        for name in LimitName::ALL {
            check(name, self.get(name))?;
        }
        self.check_cap()
    }

    fn check_cap(&self) -> Result<(), LimitError> {
        let cap = self.get(LimitName::WorkerMemoryCap);
        let memory = self.get(LimitName::Memory);
        if cap < memory.saturating_mul(MEMORY_CAP_FACTOR) {
            return Err(LimitError::CapBelowMemory { cap, memory });
        }
        let table = self.get(LimitName::MacroTableBytes);
        if table > cap {
            return Err(LimitError::TableAboveCap { table, cap });
        }
        Ok(())
    }

    fn put(&mut self, name: LimitName, value: u64) {
        let v = &mut self.0;
        let slot = match name {
            LimitName::FileSize => &mut v.file_size,
            LimitName::Work => &mut v.work,
            LimitName::Memory => &mut v.memory,
            LimitName::ArchiveDepth => &mut v.archive_depth,
            LimitName::DecompressionRatio => &mut v.decompression_ratio,
            LimitName::TotalEntries => &mut v.total_entries,
            LimitName::TotalDecompressedBytes => &mut v.total_decompressed_bytes,
            LimitName::Watchdog => &mut v.watchdog,
            LimitName::RunWatchdog => &mut v.run_watchdog,
            LimitName::WorkerMemoryCap => &mut v.worker_memory_cap,
            LimitName::EntryFacts => &mut v.entry_facts,
            LimitName::EntryMessageBytes => &mut v.entry_message_bytes,
            LimitName::MacroTableBytes => &mut v.macro_table_bytes,
            LimitName::SyntaxDepth => &mut v.syntax_depth,
        };
        *slot = value;
    }

    pub fn with(mut self, name: LimitName, value: u64) -> Result<Self, LimitError> {
        self.set(name, value)?;
        Ok(self)
    }
}

impl TryFrom<LimitValues> for Limits {
    type Error = LimitError;

    fn try_from(values: LimitValues) -> Result<Self, LimitError> {
        let limits = Limits(values);
        limits.validate()?;
        Ok(limits)
    }
}

impl From<Limits> for LimitValues {
    fn from(limits: Limits) -> Self {
        limits.0
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits(LimitValues {
            file_size: LimitName::FileSize.default_value(),
            work: LimitName::Work.default_value(),
            memory: LimitName::Memory.default_value(),
            archive_depth: LimitName::ArchiveDepth.default_value(),
            decompression_ratio: LimitName::DecompressionRatio.default_value(),
            total_entries: LimitName::TotalEntries.default_value(),
            total_decompressed_bytes: LimitName::TotalDecompressedBytes.default_value(),
            watchdog: LimitName::Watchdog.default_value(),
            run_watchdog: LimitName::RunWatchdog.default_value(),
            worker_memory_cap: LimitName::WorkerMemoryCap.default_value(),
            entry_facts: LimitName::EntryFacts.default_value(),
            entry_message_bytes: LimitName::EntryMessageBytes.default_value(),
            macro_table_bytes: LimitName::MacroTableBytes.default_value(),
            syntax_depth: LimitName::SyntaxDepth.default_value(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_enum_order_and_fields() {
        assert_eq!(LIMITS.len(), LimitName::ALL.len());
        let defaults = Limits::default();
        let json = serde_json::to_value(&defaults).unwrap();
        let fields = json.as_object().unwrap();
        assert_eq!(fields.len(), LimitName::ALL.len());
        for (i, name) in LimitName::ALL.into_iter().enumerate() {
            assert_eq!(LIMITS[i].name, name);
            assert_eq!(LimitName::from_flag(name.flag()), Some(name));
            assert!(name.default_value() > 0);
            let key = serde_json::to_value(name).unwrap();
            let key = key.as_str().unwrap();
            assert_eq!(fields[key].as_u64(), Some(name.default_value()), "{key}");
            assert_eq!(key.replace('_', "-"), name.flag());
        }
    }

    #[test]
    fn set_then_get() {
        let mut limits = Limits::default();
        let value = |i: usize, name: LimitName| match name {
            LimitName::WorkerMemoryCap => 1 << 40,
            _ => i as u64 + 1,
        };
        for (i, name) in LimitName::ALL.into_iter().enumerate() {
            limits.set(name, value(i, name)).unwrap();
        }
        for (i, name) in LimitName::ALL.into_iter().enumerate() {
            assert_eq!(limits.get(name), value(i, name));
        }
        let json = serde_json::to_string(&limits).unwrap();
        assert_eq!(serde_json::from_str::<Limits>(&json).unwrap(), limits);
    }

    #[test]
    fn e17_zero_is_refused_by_set_and_by_deserialization() {
        for name in LimitName::ALL {
            let mut limits = Limits::default();
            assert_eq!(limits.set(name, 0), Err(LimitError::Zero(name)));
            assert_eq!(limits, Limits::default());

            let mut json = serde_json::to_value(Limits::default()).unwrap();
            let key = serde_json::to_value(name).unwrap();
            json[key.as_str().unwrap()] = serde_json::Value::from(0);
            assert!(serde_json::from_value::<Limits>(json).is_err(), "{name:?}");
        }
        let mut json = serde_json::to_value(Limits::default()).unwrap();
        json["extra"] = serde_json::Value::from(1);
        assert!(serde_json::from_value::<Limits>(json).is_err());
    }

    #[test]
    fn the_macro_table_cap_fits_the_worker_cap() {
        let cap = LimitName::WorkerMemoryCap.default_value();
        let at = Limits::default()
            .with(LimitName::MacroTableBytes, cap)
            .unwrap();
        assert_eq!(at.validate(), Ok(()));
        let above = Limits::default()
            .with(LimitName::MacroTableBytes, cap + 1)
            .unwrap();
        assert_eq!(
            above.validate(),
            Err(LimitError::TableAboveCap {
                table: cap + 1,
                cap
            })
        );
        assert_eq!(LimitName::MacroTableBytes.spec().scope, LimitScope::RunWide);
    }

    #[test]
    fn entry_message_bytes_fits_the_frame_prefix() {
        let max = u64::from(u32::MAX);
        let mut limits = Limits::default();
        assert_eq!(limits.set(LimitName::EntryMessageBytes, max), Ok(()));
        assert_eq!(
            limits.set(LimitName::EntryMessageBytes, max + 1),
            Err(LimitError::AboveMax {
                name: LimitName::EntryMessageBytes,
                max
            })
        );
        assert_eq!(limits.get(LimitName::EntryMessageBytes), max);
        assert_eq!(
            limits.set(LimitName::TotalDecompressedBytes, max + 1),
            Ok(())
        );

        let mut json = serde_json::to_value(Limits::default()).unwrap();
        json["entry_message_bytes"] = serde_json::Value::from(max + 1);
        assert!(serde_json::from_value::<Limits>(json).is_err());
    }
}
