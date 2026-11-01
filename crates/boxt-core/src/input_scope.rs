use serde::{Deserialize, Serialize};

closed_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum InputScope {
        SourceOnly,
        BinariesOnly,
        PackagesOnly,
        SourceAndBinaries,
        SourceAndPackages,
        BinariesAndPackages,
        SourceBinariesAndPackages,
        ConfigurationOnly,
    }
}

impl InputScope {
    pub const fn of(source: bool, binaries: bool, packages: bool) -> InputScope {
        match (source, binaries, packages) {
            (true, false, false) => InputScope::SourceOnly,
            (false, true, false) => InputScope::BinariesOnly,
            (false, false, true) => InputScope::PackagesOnly,
            (true, true, false) => InputScope::SourceAndBinaries,
            (true, false, true) => InputScope::SourceAndPackages,
            (false, true, true) => InputScope::BinariesAndPackages,
            (true, true, true) => InputScope::SourceBinariesAndPackages,
            (false, false, false) => InputScope::ConfigurationOnly,
        }
    }
}

pub const COMPILE_DATABASE_TEXT: &str = "A compile database limited the source files scanned to the units it lists; every header in the tree was scanned, and some may not be in the build.";

const fn warning(scope: InputScope) -> &'static str {
    match scope {
        InputScope::SourceOnly => {
            "Source only: results describe the code, not the built program. Add binaries or packages for a richer result."
        }
        InputScope::BinariesOnly => {
            "Binaries only: results describe what the binaries link and contain, not whether a call is reached or how they are installed. Add packages or source for a richer result."
        }
        InputScope::PackagesOnly => {
            "Packages only: results describe the install scripts, service definitions, configs and binaries inside. Add source for findings by file, line and guard condition."
        }
        InputScope::SourceAndBinaries => {
            "Source and binaries: each finding names the input that showed it; nothing is inferred across inputs. Add packages for install context."
        }
        InputScope::SourceAndPackages => {
            "Source and packages: each finding names the input that showed it; nothing is inferred across inputs."
        }
        InputScope::BinariesAndPackages => {
            "Binaries and packages: each finding names the input that showed it; nothing is inferred across inputs. Add source for findings by file, line and guard condition."
        }
        InputScope::SourceBinariesAndPackages => {
            "Source, binaries and packages: each finding names the input that showed it; nothing is inferred across inputs."
        }
        InputScope::ConfigurationOnly => {
            "Configuration only: results describe service definitions and config files. Add source, binaries or packages for a richer result."
        }
    }
}

pub fn text(scope: InputScope, compile_database_used: bool) -> String {
    let mut text = warning(scope).to_owned();
    if compile_database_used {
        text.push(' ');
        text.push_str(COMPILE_DATABASE_TEXT);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mix_has_its_own_row() {
        let mut seen = Vec::new();
        for bits in 0u8..8 {
            let scope = InputScope::of(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            assert!(!seen.contains(&scope));
            seen.push(scope);
        }
        assert_eq!(seen.len(), InputScope::ALL.len());
        for scope in InputScope::ALL {
            assert!(seen.contains(&scope));
        }
    }

    #[test]
    fn every_row_is_the_prd_text() {
        let cdb = "A compile database limited the source files scanned to the units it lists; every header in the tree was scanned, and some may not be in the build.";
        assert_eq!(COMPILE_DATABASE_TEXT, cdb);
        for (scope, row) in [
            (
                InputScope::SourceOnly,
                "Source only: results describe the code, not the built program. Add binaries or packages for a richer result.",
            ),
            (
                InputScope::BinariesOnly,
                "Binaries only: results describe what the binaries link and contain, not whether a call is reached or how they are installed. Add packages or source for a richer result.",
            ),
            (
                InputScope::PackagesOnly,
                "Packages only: results describe the install scripts, service definitions, configs and binaries inside. Add source for findings by file, line and guard condition.",
            ),
            (
                InputScope::SourceAndBinaries,
                "Source and binaries: each finding names the input that showed it; nothing is inferred across inputs. Add packages for install context.",
            ),
            (
                InputScope::SourceAndPackages,
                "Source and packages: each finding names the input that showed it; nothing is inferred across inputs.",
            ),
            (
                InputScope::BinariesAndPackages,
                "Binaries and packages: each finding names the input that showed it; nothing is inferred across inputs. Add source for findings by file, line and guard condition.",
            ),
            (
                InputScope::SourceBinariesAndPackages,
                "Source, binaries and packages: each finding names the input that showed it; nothing is inferred across inputs.",
            ),
            (
                InputScope::ConfigurationOnly,
                "Configuration only: results describe service definitions and config files. Add source, binaries or packages for a richer result.",
            ),
        ] {
            assert_eq!(text(scope, false), row);
            assert_eq!(text(scope, true), format!("{row} {cdb}"));
        }
    }

    #[test]
    fn source_only_is_the_prd_text() {
        assert_eq!(
            text(InputScope::SourceOnly, false),
            "Source only: results describe the code, not the built program. Add binaries or packages for a richer result."
        );
        assert_eq!(
            text(InputScope::SourceOnly, true),
            format!(
                "{} {COMPILE_DATABASE_TEXT}",
                warning(InputScope::SourceOnly)
            )
        );
    }
}
