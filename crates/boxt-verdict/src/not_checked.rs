use boxt_core::SortedVec;
use boxt_core::message::CollectedFile;
use boxt_core::outcome::FileOutcome;
use boxt_core::verdict::{NotCheckedEntry, Observation};

pub(crate) fn entries(
    files: &SortedVec<CollectedFile>,
    observations: &SortedVec<Observation>,
) -> SortedVec<NotCheckedEntry> {
    let mut out = Vec::new();
    for file in files.iter() {
        match file.outcome.file {
            FileOutcome::NotChecked { .. } | FileOutcome::Aggregate { .. } => {
                out.push(NotCheckedEntry::File {
                    location: file.location.clone(),
                    outcome: file.outcome.clone(),
                });
            }
            FileOutcome::Read => {
                for region in &file.regions {
                    out.push(NotCheckedEntry::Region {
                        location: file.location.clone(),
                        first_line: region.first_line,
                        last_line: region.last_line,
                        reason: region.reason,
                    });
                }
            }
            FileOutcome::OutOfScope { .. } => {}
        }
    }
    for observation in observations.iter() {
        if let Observation::DynamicLoad { location, symbol } = observation {
            out.push(NotCheckedEntry::DynamicLoad {
                location: location.clone(),
                symbol: symbol.clone(),
            });
        }
    }
    out.into()
}
