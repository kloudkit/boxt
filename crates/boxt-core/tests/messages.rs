mod common;

use std::collections::BTreeSet;

use boxt_core::Text;
use boxt_core::compile_db::CompileUnit;
use boxt_core::fact::{Fact, FactKind, FileKind};
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{InputRef, RelPath, Scope};
use boxt_core::message::{
    CanonicalKey, CollectorMsg, Decoded, EntryBudget, EntryPath, FRAME_PREFIX_LEN, FrameError, Job,
    JobFile, JobKind, MacroDef, MacroDefError, MacroTable, Note, Region, RegionReason, WorkerMsg,
    decode_frame, encode_compile_units, encode_definitions, encode_frame, encode_macro_table,
    faulting_entry, resume_skips,
};
use boxt_core::outcome::{Details, FileOutcome, NotCheckedReason};

use common::*;

const MAX: u64 = 64 * 1024 * 1024;

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], max: u64) -> Result<T, FrameError> {
    let mut reader = bytes;
    let decoded = decode_frame::<T, _>(&mut reader, max)?.expect("a frame");
    Ok(decoded.msg)
}

fn round_trip<T>(msg: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let frame = encode_frame(msg, MAX).unwrap();
    let mut reader = frame.as_slice();
    let Decoded { msg, payload_len } = decode_frame(&mut reader, MAX).unwrap().unwrap();
    assert!(reader.is_empty());
    assert_eq!(payload_len as usize, frame.len() - FRAME_PREFIX_LEN);
    msg
}

#[test]
fn every_collector_message_round_trips() {
    let msgs = all_collector_msgs();
    let seen: BTreeSet<&str> = msgs.iter().map(collector_variant).collect();
    assert_eq!(seen, BTreeSet::from(["settings", "macros", "job"]));
    for msg in &msgs {
        assert_eq!(&round_trip(msg), msg);
    }
}

#[test]
fn every_worker_message_round_trips() {
    let msgs = all_worker_msgs();
    let seen: BTreeSet<&str> = msgs.iter().map(worker_variant).collect();
    assert_eq!(
        seen,
        BTreeSet::from([
            "opened",
            "facts",
            "regions",
            "notes",
            "entry_done",
            "compile_units",
            "definitions",
            "job_done"
        ])
    );
    let kinds: BTreeSet<FactKind> = msgs
        .iter()
        .filter_map(|m| match m {
            WorkerMsg::Facts { facts, .. } => Some(facts.iter().map(Fact::kind)),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(kinds, FactKind::ALL.into_iter().collect());
    for msg in &msgs {
        assert_eq!(&round_trip(msg), msg);
    }
}

#[test]
fn a_stream_of_frames_decodes_in_order_then_ends() {
    let msgs = all_worker_msgs();
    let mut stream = Vec::new();
    for msg in &msgs {
        stream.extend(encode_frame(msg, MAX).unwrap());
    }
    let mut reader = stream.as_slice();
    for msg in &msgs {
        let decoded = decode_frame::<WorkerMsg, _>(&mut reader, MAX)
            .unwrap()
            .unwrap();
        assert_eq!(&decoded.msg, msg);
    }
    assert_eq!(decode_frame::<WorkerMsg, _>(&mut reader, MAX), Ok(None));
}

#[test]
fn frame_layout_is_u32_le_length_then_json() {
    let msg = WorkerMsg::JobDone { outcome: None };
    let frame = encode_frame(&msg, MAX).unwrap();
    let len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
    assert_eq!(len, frame.len() - 4);
    assert_eq!(&frame[4..], br#"{"job_done":{"outcome":null}}"#);
}

#[test]
fn bad_frames_are_frame_errors() {
    let frame = encode_frame(&WorkerMsg::JobDone { outcome: None }, MAX).unwrap();

    assert_eq!(
        decode::<WorkerMsg>(&frame[..frame.len() - 1], MAX),
        Err(FrameError::Truncated)
    );
    assert_eq!(
        decode::<WorkerMsg>(&frame[..2], MAX),
        Err(FrameError::Truncated)
    );

    let limits = Limits::default();
    let cap = limits.get(LimitName::EntryMessageBytes);
    let mut oversized = Vec::new();
    oversized.extend_from_slice(&((cap + 1) as u32).to_le_bytes());
    oversized.extend_from_slice(b"{}");
    assert_eq!(
        decode::<WorkerMsg>(&oversized, cap),
        Err(FrameError::TooLong {
            len: cap + 1,
            max: cap
        })
    );

    let mut garbage = Vec::new();
    garbage.extend_from_slice(&5u32.to_le_bytes());
    garbage.extend_from_slice(b"{\"x\":");
    assert_eq!(decode::<WorkerMsg>(&garbage, MAX), Err(FrameError::Decode));

    let mut wrong_type = Vec::new();
    let payload = br#"{"no_such_message":{}}"#;
    wrong_type.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wrong_type.extend_from_slice(payload);
    assert_eq!(
        decode::<WorkerMsg>(&wrong_type, MAX),
        Err(FrameError::Decode)
    );

    let mut huge_claim = Vec::new();
    huge_claim.extend_from_slice(&u32::MAX.to_le_bytes());
    huge_claim.extend_from_slice(b"{}");
    assert_eq!(
        decode::<WorkerMsg>(&huge_claim, u64::from(u32::MAX)),
        Err(FrameError::Truncated)
    );

    assert!(matches!(
        encode_frame(&WorkerMsg::JobDone { outcome: None }, 4),
        Err(FrameError::TooLong { .. })
    ));
}

#[test]
fn io_errors_are_frame_errors() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }
    assert_eq!(
        decode_frame::<WorkerMsg, _>(&mut Broken, MAX),
        Err(FrameError::Io(std::io::ErrorKind::BrokenPipe))
    );
}

#[test]
fn v7_frame_cap_boundary() {
    let msg = WorkerMsg::JobDone { outcome: None };
    let exact = encode_frame(&msg, MAX).unwrap();
    let len = (exact.len() - FRAME_PREFIX_LEN) as u64;

    assert_eq!(encode_frame(&msg, len).unwrap(), exact);
    assert_eq!(
        encode_frame(&msg, len - 1),
        Err(FrameError::TooLong { len, max: len - 1 })
    );
    assert_eq!(decode::<WorkerMsg>(&exact, len), Ok(msg));
    assert_eq!(
        decode::<WorkerMsg>(&exact, len - 1),
        Err(FrameError::TooLong { len, max: len - 1 })
    );
}

#[test]
fn collector_messages_cannot_carry_corpus_data() {
    let json = serde_json::to_value(CollectorMsg::Settings(worker_settings())).unwrap();
    let settings = json["settings"].as_object().unwrap();
    let keys: BTreeSet<&str> = settings.keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["limits", "encodings"]));
}

fn small_limits(facts: u64, bytes: u64) -> Limits {
    Limits::default()
        .with(LimitName::EntryFacts, facts)
        .unwrap()
        .with(LimitName::EntryMessageBytes, bytes)
        .unwrap()
}

fn facts_msg(n: usize) -> WorkerMsg {
    WorkerMsg::Facts {
        entry: EntryPath::from([1]),
        facts: all_facts().into_iter().cycle().take(n).collect(),
    }
}

#[test]
fn entry_over_either_cap_is_not_checked_naming_the_limit() {
    let limits = small_limits(10, 1000);

    let mut facts = EntryBudget::new(&limits);
    assert_eq!(facts.charge_frame(&facts_msg(10), 100), Ok(()));
    assert_eq!(facts.exceeded_outcome(), None);
    assert_eq!(
        facts.charge_frame(&facts_msg(1), 0),
        Err(LimitName::EntryFacts)
    );
    assert_eq!(
        facts.exceeded_outcome(),
        Some(FileOutcome::NotChecked {
            reason: NotCheckedReason::LimitExceeded(LimitName::EntryFacts),
            details: Details::Measured {
                value: 11,
                limit: 10
            },
        })
    );

    let mut bytes = EntryBudget::new(&limits);
    let notes = WorkerMsg::Notes {
        entry: EntryPath::from([1]),
        notes: vec![Note::MissingSymbols],
    };
    assert_eq!(
        bytes.charge_frame(&notes, 1001),
        Err(LimitName::EntryMessageBytes)
    );
    assert_eq!(
        bytes.exceeded_outcome(),
        Some(FileOutcome::NotChecked {
            reason: NotCheckedReason::LimitExceeded(LimitName::EntryMessageBytes),
            details: Details::Measured {
                value: 1001,
                limit: 1000
            },
        })
    );
}

#[test]
fn v7_entry_budget_boundaries() {
    let limits = small_limits(3, 500);

    let mut at_cap = EntryBudget::new(&limits);
    assert_eq!(at_cap.charge_frame(&facts_msg(3), 500), Ok(()));
    assert_eq!(at_cap.exceeded(), None);

    let mut one_fact_over = EntryBudget::new(&limits);
    assert_eq!(
        one_fact_over.charge_frame(&facts_msg(4), 1),
        Err(LimitName::EntryFacts)
    );

    let mut one_byte_over = EntryBudget::new(&limits);
    let regions = WorkerMsg::Regions {
        entry: EntryPath::from([1]),
        regions: vec![Region {
            first_line: 1,
            last_line: 2,
            reason: RegionReason::Unparseable,
        }],
    };
    assert_eq!(one_byte_over.charge_frame(&regions, 250), Ok(()));
    assert_eq!(one_byte_over.charge_frame(&regions, 250), Ok(()));
    assert_eq!(
        one_byte_over.charge_frame(&regions, 1),
        Err(LimitName::EntryMessageBytes)
    );
}

#[test]
fn b10_only_entry_payload_frames_are_charged() {
    let limits = small_limits(1, 10);
    let mut budget = EntryBudget::new(&limits);
    let uncharged = [
        opened(EntryPath::from([1])),
        WorkerMsg::EntryDone {
            entry: EntryPath::from([1]),
            outcome: all_outcomes().remove(0),
            bytes: 1_000_000,
        },
        WorkerMsg::CompileUnits { entries: vec![] },
        WorkerMsg::JobDone { outcome: None },
    ];
    for msg in &uncharged {
        assert_eq!(budget.charge_frame(msg, 1_000_000), Ok(()));
    }
    assert_eq!(budget.exceeded(), None);
}

#[test]
fn b10_a_frame_too_long_to_send_becomes_entry_message_bytes() {
    let limits = small_limits(1_000_000, 200);
    let cap = limits.get(LimitName::EntryMessageBytes);
    let msg = facts_msg(13);
    let Err(FrameError::TooLong { len, .. }) = encode_frame(&msg, cap) else {
        panic!("the fixture must be over the frame cap");
    };
    let mut budget = EntryBudget::new(&limits);
    assert_eq!(
        budget.charge_frame(&msg, len),
        Err(LimitName::EntryMessageBytes)
    );
    let Some(FileOutcome::NotChecked { reason, details }) = budget.exceeded_outcome() else {
        panic!("over the cap");
    };
    assert_eq!(
        reason,
        NotCheckedReason::LimitExceeded(LimitName::EntryMessageBytes)
    );
    assert_eq!(
        details,
        Details::Measured {
            value: len,
            limit: cap
        }
    );
}

fn unit(i: usize) -> CompileUnit {
    CompileUnit {
        directory: Text::from("/build"),
        file: Text::from(format!("../src/file-{i:04}.c")),
    }
}

#[test]
fn b9_compile_units_split_across_frames_within_the_cap() {
    let units: Vec<CompileUnit> = (0..500).map(unit).collect();
    let cap = 1024;
    let frames = encode_compile_units(&units, cap).unwrap();
    assert!(frames.len() > 1);
    let mut stream = Vec::new();
    for frame in &frames {
        assert!((frame.len() - FRAME_PREFIX_LEN) as u64 <= cap);
        stream.extend_from_slice(frame);
    }
    stream.extend(encode_frame(&WorkerMsg::JobDone { outcome: None }, cap).unwrap());

    let mut reader = stream.as_slice();
    let mut received = Vec::new();
    loop {
        match decode_frame::<WorkerMsg, _>(&mut reader, cap).unwrap() {
            Some(Decoded {
                msg: WorkerMsg::CompileUnits { entries },
                ..
            }) => received.extend(entries),
            Some(Decoded {
                msg: WorkerMsg::JobDone { outcome: None },
                ..
            }) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(received, units);

    let exact = encode_compile_units(&units[..1], cap).unwrap();
    let exact_len = (exact[0].len() - FRAME_PREFIX_LEN) as u64;
    assert_eq!(encode_compile_units(&units[..1], exact_len).unwrap(), exact);
    assert!(matches!(
        encode_compile_units(&units[..1], exact_len - 1),
        Err(FrameError::TooLong { .. })
    ));

    let empty = encode_compile_units(&[], cap).unwrap();
    assert_eq!(empty.len(), 1);
    assert_eq!(
        decode::<WorkerMsg>(&empty[0], cap),
        Ok(WorkerMsg::CompileUnits { entries: vec![] })
    );
}

#[test]
fn b9_packing_fills_frames_to_the_cap() {
    let units: Vec<CompileUnit> = (0..40).map(unit).collect();
    let one = encode_compile_units(&units, MAX).unwrap();
    assert_eq!(one.len(), 1);
    let whole = (one[0].len() - FRAME_PREFIX_LEN) as u64;
    assert_eq!(encode_compile_units(&units, whole).unwrap().len(), 1);
    assert_eq!(encode_compile_units(&units, whole - 1).unwrap().len(), 2);
}

fn paths(list: &[&[u32]]) -> Vec<EntryPath> {
    list.iter().map(|p| EntryPath::from(*p)).collect()
}

#[test]
fn faulting_entry_matrix_row() {
    let committed = paths(&[&[0], &[1], &[2, 0], &[2, 1]]);
    assert_eq!(faulting_entry(&committed, None), EntryPath::from([2, 2]));
    assert_eq!(
        faulting_entry(&committed, Some(&EntryPath::from([2, 1]))),
        EntryPath::from([2, 2])
    );
    assert_eq!(
        faulting_entry(&committed, Some(&EntryPath::from([2, 2]))),
        EntryPath::from([2, 2])
    );
}

#[test]
fn nothing_committed_faults_the_whole_job() {
    assert_eq!(faulting_entry(&[], None), EntryPath::job());
    assert_eq!(
        faulting_entry(&[], Some(&EntryPath::from([0]))),
        EntryPath::job()
    );
}

type FaultCase<'a> = (&'a [&'a [u32]], Option<&'a [u32]>, &'a [u32]);

#[test]
fn nested_fault_fixtures() {
    let cases: &[FaultCase] = &[
        (&[&[0]], None, &[1]),
        (&[&[0], &[1], &[2]], Some(&[3]), &[3]),
        (&[&[0], &[1]], Some(&[2, 0]), &[2, 0]),
        (
            &[&[0], &[1, 0], &[1, 1, 0], &[1, 1, 1]],
            Some(&[1, 1, 2]),
            &[1, 1, 2],
        ),
        (
            &[&[0], &[1, 0], &[1, 1, 0], &[1, 1, 1], &[1, 2]],
            Some(&[1, 2]),
            &[1, 3],
        ),
        (
            &[&[0], &[1, 0], &[1, 1, 0], &[1, 1, 1], &[2]],
            Some(&[2]),
            &[3],
        ),
        (&[&[0], &[2, 0], &[2, 1]], Some(&[3]), &[3]),
        (
            &[&[0], &[1], &[2, 0], &[2, 1]],
            Some(&[2, 2, 0]),
            &[2, 2, 0],
        ),
        (
            &[&[0], &[1], &[2, 0], &[2, 1], &[2, 2, 0]],
            Some(&[2, 2, 0]),
            &[2, 2, 1],
        ),
        (&[&[0], &[1, 0], &[1, 1]], Some(&[0]), &[1, 2]),
    ];
    for (committed, last_seen, expected) in cases {
        let committed = paths(committed);
        let last_seen = last_seen.map(EntryPath::from);
        assert_eq!(
            faulting_entry(&committed, last_seen.as_ref()),
            EntryPath::from(*expected),
            "committed {committed:?}, last seen {last_seen:?}"
        );
    }
}

#[test]
fn resume_skips_matrix_row() {
    let after = EntryPath::from([2, 1]);
    for skipped in [&[0][..], &[1], &[2, 0], &[2, 1], &[2, 1, 0], &[2, 1, 7, 3]] {
        assert!(
            resume_skips(&EntryPath::from(skipped), &after),
            "{skipped:?}"
        );
    }
    for sent in [&[2, 2][..], &[2, 2, 0], &[3], &[3, 0]] {
        assert!(!resume_skips(&EntryPath::from(sent), &after), "{sent:?}");
    }
}

#[test]
fn fault_then_resume_sends_each_entry_once() {
    let stream: Vec<EntryPath> = paths(&[
        &[0],
        &[1],
        &[2, 0],
        &[2, 1],
        &[2, 2],
        &[2, 3, 0],
        &[2, 3, 1],
        &[3],
    ]);
    let committed_before_fault = &stream[..4];
    let faulted = faulting_entry(committed_before_fault, Some(&stream[4]));
    assert_eq!(faulted, EntryPath::from([2, 2]));
    let resent: Vec<&EntryPath> = stream
        .iter()
        .filter(|e| !resume_skips(e, &faulted))
        .collect();
    assert_eq!(
        resent,
        vec![
            &EntryPath::from([2, 3, 0]),
            &EntryPath::from([2, 3, 1]),
            &EntryPath::from([3])
        ]
    );
    let mut all: Vec<&EntryPath> = committed_before_fault.iter().collect();
    all.push(&faulted);
    all.extend(resent);
    let unique: BTreeSet<&EntryPath> = all.iter().copied().collect();
    assert_eq!(unique.len(), all.len());
    assert_eq!(unique.len(), stream.len());
}

#[test]
fn a_fault_on_opening_a_sibling_blames_that_sibling() {
    let committed = paths(&[&[0], &[1], &[2, 0], &[2, 1]]);
    let opened = opened(EntryPath::from([3]));
    let last_seen = opened.entry().unwrap();
    let faulted = faulting_entry(&committed, Some(last_seen));
    assert_eq!(faulted, EntryPath::from([3]));

    let second_pass = paths(&[
        &[0],
        &[1],
        &[2, 0],
        &[2, 1],
        &[3],
        &[3, 0],
        &[3, 1, 0],
        &[4],
        &[5, 0],
    ]);
    let resent: Vec<&EntryPath> = second_pass
        .iter()
        .filter(|e| !resume_skips(e, &faulted))
        .collect();
    assert_eq!(
        resent,
        vec![&EntryPath::from([4]), &EntryPath::from([5, 0])]
    );
}

#[test]
fn a_fault_moving_to_the_next_child_blames_that_child() {
    let committed = paths(&[&[0], &[1], &[2, 0], &[2, 1]]);
    let opened = opened(EntryPath::from([2, 2]));
    let faulted = faulting_entry(&committed, opened.entry());
    assert_eq!(faulted, EntryPath::from([2, 2]));
    let second_pass = paths(&[
        &[0],
        &[1],
        &[2, 0],
        &[2, 1],
        &[2, 2],
        &[2, 2, 0],
        &[3],
        &[4, 0],
    ]);
    let resent: Vec<&EntryPath> = second_pass
        .iter()
        .filter(|e| !resume_skips(e, &faulted))
        .collect();
    assert_eq!(
        resent,
        vec![&EntryPath::from([3]), &EntryPath::from([4, 0])]
    );
}

#[test]
fn entry_paths_sort_lexicographically() {
    let mut v = paths(&[&[2, 1], &[10], &[2], &[0], &[2, 0, 5], &[], &[1]]);
    v.sort();
    assert_eq!(
        v,
        paths(&[&[], &[0], &[1], &[2], &[2, 0, 5], &[2, 1], &[10]])
    );
}

fn job(index: u32, path: &str) -> Job {
    Job {
        kind: JobKind::Route,
        input: InputRef {
            index,
            given: Text::from(format!("/in/{index}")),
        },
        file: JobFile::InInput(RelPath::try_from(path).unwrap()),
        resume_after: None,
    }
}

fn import_at(index: u32, path: &str, line: u32) -> Fact {
    let mut location = location(path, line, Scope::Symbol(Text::from("dlopen")));
    location.input.index = index;
    Fact::Import {
        file_kind: FileKind::Binary,
        location,
        symbol: Text::from("dlopen"),
        version: None,
    }
}

#[test]
fn b12_canonical_key_orders_input_path_entry_location() {
    let a = job(0, "b.rpm");
    let b = job(0, "c.rpm");
    let c = job(1, "a.rpm");
    let mut keys = [
        CanonicalKey::for_fact(&c, &EntryPath::from([0]), &import_at(1, "x", 1)),
        CanonicalKey::for_fact(&b, &EntryPath::from([0]), &import_at(0, "x", 1)),
        CanonicalKey::for_fact(&a, &EntryPath::from([2]), &import_at(0, "a", 1)),
        CanonicalKey::for_fact(&a, &EntryPath::from([1, 5]), &import_at(0, "z", 9)),
        CanonicalKey::for_fact(&a, &EntryPath::from([1, 5]), &import_at(0, "z", 2)),
        CanonicalKey::for_outcome(&a, &EntryPath::from([1, 5])),
    ]
    .map(Option::unwrap);
    keys.sort();
    type Row<'a> = (u32, &'a [u8], Vec<u32>, Option<u32>);
    let order: Vec<Row> = keys
        .iter()
        .map(|k| {
            (
                k.input_index,
                k.job_path.as_bytes(),
                k.entry.as_slice().to_vec(),
                k.location.as_ref().and_then(|l| l.line),
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            (0, b"b.rpm".as_slice(), vec![1, 5], None),
            (0, b"b.rpm".as_slice(), vec![1, 5], Some(2)),
            (0, b"b.rpm".as_slice(), vec![1, 5], Some(9)),
            (0, b"b.rpm".as_slice(), vec![2], Some(1)),
            (0, b"c.rpm".as_slice(), vec![0], Some(1)),
            (1, b"a.rpm".as_slice(), vec![0], Some(1)),
        ]
    );
}

fn many_defs(n: usize) -> Vec<MacroDef> {
    (0..n)
        .map(|i| macro_def(&format!("M{i:04}"), "src/config.h", i as u32 + 1, None))
        .collect()
}

fn drain<T: serde::de::DeserializeOwned>(frames: &[Vec<u8>], cap: u64) -> Vec<T> {
    let stream: Vec<u8> = frames.concat();
    let mut reader = stream.as_slice();
    let mut out = Vec::new();
    while let Some(Decoded { msg, payload_len }) = decode_frame::<T, _>(&mut reader, cap).unwrap() {
        assert!(payload_len <= cap);
        out.push(msg);
    }
    out
}

#[test]
fn definitions_split_across_frames_within_the_cap() {
    let defs = many_defs(200);
    let cap = 1024;
    let frames = encode_definitions(&defs, cap).unwrap();
    assert!(frames.len() > 1);
    let mut received = Vec::new();
    for msg in drain::<WorkerMsg>(&frames, cap) {
        match msg {
            WorkerMsg::Definitions { definitions } => received.extend(definitions),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(received, defs);

    let empty = encode_definitions(&[], cap).unwrap();
    assert_eq!(
        drain::<WorkerMsg>(&empty, cap),
        vec![WorkerMsg::Definitions {
            definitions: vec![]
        }]
    );
}

#[test]
fn a_definition_over_the_cap_is_too_long() {
    let defs = many_defs(1);
    let one = encode_definitions(&defs, MAX).unwrap();
    let len = (one[0].len() - FRAME_PREFIX_LEN) as u64;
    let fits = encode_definitions(&defs, len + 64).unwrap();
    assert_eq!(fits.len(), 1);
    assert!(matches!(
        encode_definitions(&defs, len - 1),
        Err(FrameError::TooLong { .. })
    ));
    let table = MacroTable::from(defs);
    let table_frames = encode_macro_table(u32::MAX, &table, len + 64).unwrap();
    assert_eq!(table_frames.len(), 1);
}

#[test]
fn any_definition_a_worker_can_send_fits_the_table_frame() {
    for n in [1, 7, 60] {
        let defs = many_defs(n);
        let job = Job {
            kind: JobKind::Definitions,
            input: input(),
            file: JobFile::InInput(RelPath::try_from("src/config.h").unwrap()),
            resume_after: None,
        };
        for cap in [400, 512, 1024, 4096] {
            if encode_definitions(&defs, cap).is_err() {
                continue;
            }
            for def in &defs {
                assert_eq!(def.validate(&job, cap), Ok(()));
            }
            let table = MacroTable::from(defs.clone());
            assert!(encode_macro_table(u32::MAX, &table, cap).is_ok());
        }
    }
}

#[test]
fn the_macro_table_splits_by_tree_and_concatenates_in_order() {
    let mut defs = many_defs(150);
    defs.extend(macro_defs());
    let table = MacroTable::from(defs);
    let cap = 1024;
    let frames = encode_macro_table(3, &table, cap).unwrap();
    assert!(frames.len() > 1);
    let mut received = Vec::new();
    let mut ends = Vec::new();
    for msg in drain::<CollectorMsg>(&frames, cap) {
        match msg {
            CollectorMsg::Macros {
                tree,
                definitions,
                end,
            } => {
                assert_eq!(tree, 3);
                received.extend(definitions.into_vec());
                ends.push(end);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(received, table.clone().into_vec());
    assert!(received.windows(2).all(|w| w[0] <= w[1]));
    let last = ends.pop();
    assert_eq!(last, Some(true));
    assert!(ends.iter().all(|end| !end));
}

#[test]
fn macro_table_orders_by_name_bytes_then_location_and_keeps_duplicates() {
    let table = macro_table();
    let order: Vec<(String, String, u32)> = table
        .iter()
        .map(|d| {
            (
                String::from_utf8_lossy(d.name.as_bytes()).into_owned(),
                String::from_utf8_lossy(d.location.path.as_ref().unwrap().as_bytes()).into_owned(),
                d.location.line.unwrap(),
            )
        })
        .collect();
    let row = |n: &str, p: &str, l| (n.to_owned(), p.to_owned(), l);
    assert_eq!(
        order,
        vec![
            row("EMPTY", "src/config.h", 15),
            row("FOO", "src/a.c", 2),
            row("FOO", "src/config.h", 3),
            row("FOO", "src/config.h", 9),
            row("LOG", "src/config.h", 12),
            row("RAW", "src/config.h", 16),
            row("TRACE", "src/config.h", 14),
        ]
    );
    let twice = MacroTable::from(vec![
        macro_def("FOO", "src/config.h", 3, None),
        macro_def("FOO", "src/config.h", 3, None),
    ]);
    assert_eq!(twice.len(), 2);
    let json = serde_json::to_string(&table).unwrap();
    let back: MacroTable = serde_json::from_str(&json).unwrap();
    assert_eq!(back, table);
    let mut reversed = table.clone().into_vec();
    reversed.reverse();
    let resorted: MacroTable =
        serde_json::from_str(&serde_json::to_string(&reversed).unwrap()).unwrap();
    assert_eq!(resorted, table);
}

#[test]
fn macro_definitions_are_checked_against_their_job() {
    let job = Job {
        kind: JobKind::Definitions,
        input: input(),
        file: JobFile::InInput(RelPath::try_from("src/config.h").unwrap()),
        resume_after: None,
    };
    let good = macro_def("FOO", "src/config.h", 3, None);
    assert_eq!(good.validate(&job, MAX), Ok(()));
    let with = |f: &dyn Fn(&mut MacroDef)| {
        let mut def = good.clone();
        f(&mut def);
        def.validate(&job, MAX)
    };
    assert_eq!(
        with(&|d| d.location.line = None),
        Err(MacroDefError::NoLine)
    );
    assert_eq!(
        with(&|d| d.location.line = Some(0)),
        Err(MacroDefError::NoLine)
    );
    assert_eq!(
        with(&|d| d.location.path = None),
        Err(MacroDefError::NoPath)
    );
    assert_eq!(
        with(&|d| d.location.frames = frames()),
        Err(MacroDefError::InContainer)
    );
    assert_eq!(
        with(&|d| d.location.path = Some(RelPath::try_from("src/other.h").unwrap())),
        Err(MacroDefError::OtherFile)
    );
    assert_eq!(
        with(&|d| d.location.input.index = 1),
        Err(MacroDefError::OtherFile)
    );
    assert_eq!(
        with(&|d| d.location.scope = function_scope()),
        Err(MacroDefError::NotFileScope)
    );
    let route = Job {
        kind: JobKind::Route,
        ..job.clone()
    };
    assert_eq!(good.validate(&route, MAX), Err(MacroDefError::OtherFile));
}

#[test]
fn definitions_jobs_share_the_source_jobs_file_and_never_resume() {
    let file = JobFile::InInput(RelPath::try_from("src/main.c").unwrap());
    let job = |kind, file: JobFile, resume_after| Job {
        kind,
        input: input(),
        file,
        resume_after,
    };
    assert!(job(JobKind::Definitions, file.clone(), None).is_well_formed());
    assert!(
        job(
            JobKind::Definitions,
            JobFile::WholeInput(RelPath::try_from("main.c").unwrap()),
            None
        )
        .is_well_formed()
    );
    assert!(
        !job(
            JobKind::Definitions,
            file.clone(),
            Some(EntryPath::from([1]))
        )
        .is_well_formed()
    );
    assert!(
        !job(
            JobKind::Definitions,
            JobFile::Setting(Text::from("/x.json")),
            None
        )
        .is_well_formed()
    );
    let definitions = job(JobKind::Definitions, file.clone(), None);
    assert_eq!(
        CanonicalKey::for_outcome(&definitions, &EntryPath::job()),
        None
    );
    assert_eq!(
        CanonicalKey::for_fact(
            &definitions,
            &EntryPath::job(),
            &sample_fact(FactKind::Call)
        ),
        None
    );
    assert!(
        CanonicalKey::for_outcome(&job(JobKind::Route, file, None), &EntryPath::job()).is_some()
    );
    let mut budget = EntryBudget::new(&small_limits(1, 1));
    let msg = WorkerMsg::Definitions {
        definitions: macro_defs(),
    };
    assert_eq!(budget.charge_frame(&msg, 1), Ok(()));
    assert_eq!(
        budget.charge_frame(&msg, 1),
        Err(LimitName::EntryMessageBytes)
    );
    assert_eq!(msg.entry(), None);
}

#[test]
fn each_worker_message_belongs_to_its_job_kind() {
    for msg in all_worker_msgs() {
        let kinds: Vec<JobKind> = [
            JobKind::Route,
            JobKind::CompileDatabase,
            JobKind::Definitions,
        ]
        .into_iter()
        .filter(|&k| msg.allowed_in(k))
        .collect();
        let expected = match (worker_variant(&msg), &msg) {
            ("job_done", WorkerMsg::JobDone { outcome: None }) => {
                vec![JobKind::Route, JobKind::CompileDatabase]
            }
            ("job_done", _) => vec![
                JobKind::Route,
                JobKind::CompileDatabase,
                JobKind::Definitions,
            ],
            ("compile_units", _) => vec![JobKind::CompileDatabase],
            ("definitions", _) => vec![JobKind::Definitions],
            ("notes", WorkerMsg::Notes { notes, .. })
                if notes.contains(&Note::DroppedDefinitions(2)) =>
            {
                vec![JobKind::Definitions]
            }
            _ => vec![JobKind::Route],
        };
        assert_eq!(kinds, expected, "{msg:?}");
    }
    let dropped = WorkerMsg::Notes {
        entry: EntryPath::from([1]),
        notes: vec![Note::DroppedDefinitions(1)],
    };
    assert!(!dropped.allowed_in(JobKind::Definitions));
    assert!(!dropped.allowed_in(JobKind::Route));
}
