use crate::message::RegionReason;
use crate::outcome::{
    FaultKind, IoKind, LinkTarget, NotCheckedReason, OutOfScopeCause, OwnFileRole, SpecialKind,
};
use crate::verdict::{Confidence, Tier};

pub const HOST_DEPENDENT_TEXT: &str = "Some files were ended by a time or memory limit of this host; the result may differ on another host.";

pub const HOST_DEPENDENT_MARK: &str = "may differ on another host";

pub const PROJECT_FILE: &str = "project file";

pub const PROJECT_FILE_NONE_FOUND: &str = "none found";

pub const PROJECT_FILE_NAMED: &str = "named by --project";

pub const ABOVE_VCS_ROOT_TEXT: &str = "The project file lies above the version-control root of the first input; check that it is meant for this tree.";

pub fn macro_table_cut() -> String {
    format!(
        "macro definitions past the {} limit were not expanded",
        crate::limits::LimitName::MacroTableBytes.flag()
    )
}

pub const fn tier(tier: Tier) -> &'static str {
    match tier {
        Tier::Blocked => "blocked",
        Tier::NeedsWindows => "needs Windows",
        Tier::NeedsCodeChange => "needs code change",
        Tier::ReadyWithConfig => "ready with config",
        Tier::Ready => "ready",
    }
}

pub const fn confidence(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Found => "found",
        Confidence::Suspected => "suspected",
    }
}

pub const fn region_reason(reason: RegionReason) -> &'static str {
    match reason {
        RegionReason::Unparseable => "unparseable region",
    }
}

pub const fn region_reason_plural(reason: RegionReason) -> &'static str {
    match reason {
        RegionReason::Unparseable => "unparseable regions",
    }
}

pub fn not_checked_reason(reason: NotCheckedReason) -> String {
    match reason {
        NotCheckedReason::NoReader => "no reader".to_owned(),
        NotCheckedReason::EncodingUndeclared => "encoding undeclared".to_owned(),
        NotCheckedReason::InvalidForDeclaredEncoding => {
            "invalid for its declared encoding".to_owned()
        }
        NotCheckedReason::Unparseable => "unparseable".to_owned(),
        NotCheckedReason::UnsupportedInterpreter => "unsupported interpreter".to_owned(),
        NotCheckedReason::NoDecoder => "no decoder".to_owned(),
        NotCheckedReason::LimitExceeded(limit) => format!("over the {} limit", limit.flag()),
        NotCheckedReason::Fault(kind) => format!("reader fault: {}", fault(kind)),
        NotCheckedReason::RepeatedFault => "repeated reader fault".to_owned(),
        NotCheckedReason::SpecialFile(kind) => format!("special file: {}", special(kind)),
        NotCheckedReason::Symlink(LinkTarget::Special(kind)) => {
            format!("symbolic link to {}", special_target(kind))
        }
        NotCheckedReason::Symlink(target) => format!("symbolic link to {}", link(target)),
        NotCheckedReason::Unreadable(kind) => format!("unreadable: {}", io(kind)),
        NotCheckedReason::UnsafeEntryName => "unsafe entry name".to_owned(),
        NotCheckedReason::NotInCompileDatabase => "not in the compile database".to_owned(),
    }
}

pub const fn out_of_scope_cause(cause: &OutOfScopeCause) -> &'static str {
    match cause {
        OutOfScopeCause::Pattern(_) => "excluded by a pattern",
        OutOfScopeCause::OwnFile(role) => match role {
            OwnFileRole::ProjectFile => "boxt's project file",
            OwnFileRole::Output => "a boxt output",
            OwnFileRole::CompileDatabase => "the compile database",
            OwnFileRole::WaiverFile => "the waiver file",
            OwnFileRole::UserRules => "a user rule file",
        },
    }
}

const fn fault(kind: FaultKind) -> &'static str {
    match kind {
        FaultKind::Crash => "crash",
        FaultKind::Panic => "panic",
        FaultKind::BadFrame => "bad message",
        FaultKind::Exited => "exited early",
    }
}

const fn special(kind: SpecialKind) -> &'static str {
    match kind {
        SpecialKind::Fifo => "FIFO",
        SpecialKind::Socket => "socket",
        SpecialKind::BlockDevice => "block device",
        SpecialKind::CharDevice => "character device",
        SpecialKind::ReparsePoint => "reparse point",
        SpecialKind::Other => "other",
    }
}

const fn special_target(kind: SpecialKind) -> &'static str {
    match kind {
        SpecialKind::Fifo => "a FIFO",
        SpecialKind::Socket => "a socket",
        SpecialKind::BlockDevice => "a block device",
        SpecialKind::CharDevice => "a character device",
        SpecialKind::ReparsePoint => "a reparse point",
        SpecialKind::Other => "a special file",
    }
}

const fn link(target: LinkTarget) -> &'static str {
    match target {
        LinkTarget::File => "a file",
        LinkTarget::Directory => "a directory",
        LinkTarget::Special(_) => "a special file",
        LinkTarget::Missing => "nothing",
        LinkTarget::Loop => "a loop",
        LinkTarget::Unresolved => "an unresolved target",
    }
}

const fn io(kind: IoKind) -> &'static str {
    match kind {
        IoKind::NotFound => "not found",
        IoKind::PermissionDenied => "permission denied",
        IoKind::IsADirectory => "is a directory",
        IoKind::Other => "I/O error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_labels_are_the_prd_words() {
        let labels: Vec<&str> = Tier::WORST_TO_BEST.iter().map(|&t| tier(t)).collect();
        assert_eq!(
            labels,
            [
                "blocked",
                "needs Windows",
                "needs code change",
                "ready with config",
                "ready"
            ]
        );
    }

    #[test]
    fn a_link_to_a_special_file_names_its_kind() {
        assert_eq!(
            not_checked_reason(NotCheckedReason::Symlink(LinkTarget::Special(
                SpecialKind::Fifo
            ))),
            "symbolic link to a FIFO"
        );
        assert_eq!(
            not_checked_reason(NotCheckedReason::Symlink(LinkTarget::Special(
                SpecialKind::Other
            ))),
            "symbolic link to a special file"
        );
    }

    #[test]
    fn a_limit_reason_names_the_limit_flag() {
        use crate::limits::LimitName;
        assert_eq!(
            not_checked_reason(NotCheckedReason::LimitExceeded(LimitName::FileSize)),
            format!("over the {} limit", LimitName::FileSize.flag())
        );
    }
}
