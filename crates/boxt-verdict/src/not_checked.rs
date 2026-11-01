use boxt_core::SortedVec;
use boxt_core::message::CollectedFile;
use boxt_core::outcome::FileOutcome;
use boxt_core::verdict::{LineRange, NotCheckedEntry, Observation};

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
                let mut regions: Vec<_> = file
                    .regions
                    .iter()
                    .map(|r| {
                        (
                            r.reason,
                            LineRange {
                                first: r.first_line,
                                last: r.last_line,
                            },
                        )
                    })
                    .collect();
                regions.sort();
                for chunk in regions.chunk_by(|a, b| a.0 == b.0) {
                    out.push(NotCheckedEntry::Regions {
                        location: file.location.clone(),
                        reason: chunk[0].0,
                        ranges: chunk.iter().map(|(_, range)| *range).collect(),
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

pub(crate) fn no_reader(entries: &SortedVec<NotCheckedEntry>) -> u64 {
    entries
        .iter()
        .filter(|e| e.is_no_reader())
        .fold(0u64, |n, _| n.saturating_add(1))
}
