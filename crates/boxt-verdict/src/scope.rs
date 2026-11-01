use boxt_core::SortedVec;
use boxt_core::fact::FileKind;
use boxt_core::input_scope::InputScope;
use boxt_core::location::FrameKind;
use boxt_core::message::CollectedFile;
use boxt_core::outcome::FileOutcome;

pub(crate) fn of(files: &SortedVec<CollectedFile>) -> InputScope {
    let mut source = false;
    let mut binaries = false;
    let mut packages = false;
    for file in files.iter() {
        if !matches!(file.outcome.file, FileOutcome::Read) {
            continue;
        }
        match file.outcome.file_kind {
            Some(FileKind::Source) => source = true,
            Some(FileKind::Package) => packages = true,
            Some(FileKind::Binary) => {
                let in_package = file
                    .location
                    .frames
                    .iter()
                    .any(|f| matches!(f.kind, FrameKind::Rpm | FrameKind::Deb));
                binaries |= !in_package;
            }
            Some(FileKind::Service | FileKind::Config) | None => {}
        }
    }
    InputScope::of(source, binaries, packages)
}
