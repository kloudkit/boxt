# Limit defaults

Story 1.6 set every default in `LIMITS` (`src/limits.rs`) from a measurement on the seven
public spike code bases, and froze them on 2026-11-01. No default changes before Story 5.2
(the kill test) without an operator decision; a change records its new measurement here.

## How the defaults were measured

- **Code bases:** OpenSSH 9.9p2, ntp 4.2.8p18 and Xerces-C 3.2.5
  (`spikes/parser-spike/sources`, fetched by `fetch.sh`), net-snmp 5.9.4 and squid 6.12
  (`spikes/parser-mitigations/sources`), ACE 6.5.25 and sendmail 8.18.2
  (`spikes/parser-mitigations-t5/sources-fresh`). 17,765 walked entries; 12,040 of them
  are C/C++ candidates by name (a listed extension, or no extension), and 8,914 of those
  are C/C++ after the router's content check.
- **Commands** (release build, Linux x86_64 glibc, 12 cores, a shared host):
  - `boxt collect <code base> --measure` runs the real pipeline. It prints the walked
    entries, the largest walked file, the longest job's wall time, the run's wall time, the
    largest worker RSS, the merged macro-table bytes, and the most facts one file sent.
    Its `cut-files` line counts the files (not outcomes: an aggregate counts its files)
    each limit cut, `TotalEntries` and `RunWatchdog` included, and the trees
    `MacroTableBytes` cut.
  - `boxt measure <code base>` reads every candidate in one process exactly as the worker
    charges it: the content check and the read share one `WorkBudget`, with the tree's
    merged macro table attached. `FileSize`, `Work`, `Memory`, `SyntaxDepth`,
    `EntryFacts` and `MacroTableBytes` are raised to their maximum, so each file's own size,
    work steps, peak counted memory (`WorkBudget::peak_buffered`), deepest syntax tree
    (`WorkBudget::deepest_syntax`) and fact count are measured rather than cut. Every
    candidate is counted: it reports how many the content check rejected, how many could
    not be fetched and how many the reader did not read, then how many files each shipped
    default would cut. It also reports the largest peak counted memory per KiB of file,
    over files of at least 64 KiB.
- **Worker RSS** is each worker's `VmHWM` from `/proc`, read after every job it finishes.
  It is Linux only, and a worker killed by the OS cap, the watchdog or the run watchdog is
  never sampled; none was killed in these runs.
- **Margin rule:** a deterministic limit's default is at least 10 times the largest
  measurement, rounded up to a round value. `Memory` must also hold a file at the
  `FileSize` limit: at least the largest measured peak per byte times `FileSize`. A
  last-resort limit (wall clock, OS memory) varies by host, so it is set wider; its row
  says how.
- **Undeclared encodings:** the runs declare no encoding, so Latin-1 files are
  `EncodingUndeclared` and never read. The Story 1.5 acceptance test declares them, and
  gave no larger measurement for any limit on the same files.

## Measured

Run of 2026-11-01 on the final code. The three `collect` runs per code base (`--measure`,
plain, and `--jobs 1`) gave the same counts and digest, and wrote no file.

| Code base | Entries | C/C++ files | Largest C/C++ file (bytes) | Work | Memory (bytes) | Memory per byte | Syntax depth | Facts in one file | Macro tables (bytes) | Longest job | Run | Worker RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OpenSSH | 881 | 430 | 461,295 | 28,953 | 2,719,460 | 10.2 | 70 | 2,134 | 161,743 | 1.8 s | 3.5 s | 42.5 MB |
| ntp | 1,741 | 857 | 165,146 | 145,968 | 1,514,166 | 17.0 | 48 | 3,024 | 534,200 | 6.0 s | 10.1 s | 60.9 MB |
| Xerces-C | 2,733 | 907 | 854,807 | 10,614 | 1,348,013 | 5.8 | 107 | 3,474 | 68,179 | 1.5 s | 2.8 s | 69.8 MB |
| net-snmp | 2,245 | 1,326 | 212,222 | 9,276 | 1,061,090 | 9.1 | 109 | 1,343 | 240,081 | 6.6 s | 14.0 s | 63.3 MB |
| squid | 4,248 | 1,692 | 150,050 | 17,032 | 1,200,842 | 8.1 | 101 | 2,305 | 113,155 | 4.5 s | 7.9 s | 80.2 MB |
| ACE | 5,112 | 3,439 | 159,970 | 45,881 | 1,297,966 | 9.7 | 705 | 1,449 | 356,193 | 10.2 s | 24.1 s | 31.2 MB |
| sendmail | 805 | 263 | 214,455 | 27,646 | 1,199,366 | 6.8 | 54 | 2,595 | 180,816 | 13.9 s | 26.9 s | 29.2 MB |
| **Largest** | 5,112 | | 854,807 | 145,968 | 2,719,460 | 17.0 | 705 | 3,474 | 534,200 | 13.9 s | 26.9 s | 80.2 MB |

- Wall times are with 12 workers on a host shared with other work. An earlier run on a
  busier host reached 20.1 s for one job and 36.5 s for a run (both sendmail); the
  wall-clock defaults are set from those.
- The largest walked file of any kind was 6,342,142 bytes (net-snmp, not C/C++).
- The reader did not read 3 candidates (ntp `libntp/adjtimex.c`, squid
  `src/acl/Strategised.cc`, ACE `apps/JAWS/clients/Caching/Local_Locator.cpp`, all
  `Unparseable`), with no limit involved.

## Defaults

The share cut is out of the 8,914 C/C++ files of the seven code bases, or their 17,765
entries for a run-wide limit, with the default in place: `cut-files` from
`boxt collect --measure` and `over-default` from `boxt measure`.

| Limit | Default | Measurement that set it | Share cut | Date |
| --- | --- | --- | --- | --- |
| `FileSize` | 16 MiB | largest C/C++ file 854,807 bytes (Xerces `XMLChar.cpp`); 19.6× | 0 of 8,914 | 2026-11-01 |
| `Work` | 2,000,000 steps | most work in one file 145,968 steps (ntp); 13.7× | 0 of 8,914 | 2026-11-01 |
| `Memory` | 512 MiB | peak counted memory 2,719,460 bytes (OpenSSH); 197×. The binding rule is the `FileSize` one: the largest peak per byte is 17.0 (ntp), and 17.0 × 16 MiB is 272 MiB, so 512 MiB is 1.9× that | 0 of 8,914 | 2026-11-01 |
| `SyntaxDepth` | 10,000 levels | deepest tree 705 levels (ACE); 14× | 0 of 8,914 | 2026-11-01 |
| `EntryFacts` | 50,000 facts | most facts in one file 3,474 (Xerces); 14× | 0 of 8,914 | 2026-11-01 |
| `MacroTableBytes` | 8 MiB | largest merged table 534,200 bytes (ntp); 15.7×, below `WorkerMemoryCap` | 0 of 7 trees | 2026-11-01 |
| `TotalEntries` | 100,000 entries | most entries in one run 5,112 (ACE); 19.6× | 0 of 17,765 | 2026-11-01 |
| `Watchdog` | 600 s | longest single job 20.1 s (sendmail, 12 workers on a loaded host); 30×, so a slower CI runner stays far from it | 0 of 8,914 | 2026-11-01 |
| `RunWatchdog` | 6 h | longest run 36.5 s (sendmail, 805 entries); 590×. Sendmail is the costliest code per entry, at about 0.54 s of worker time each: 6 h fills at about 40,000 such entries on one worker, or 480,000 on twelve. So a `TotalEntries`-sized run of such code on fewer than three workers can reach it; at ACE's 82 ms per entry it fills at 263,000 entries on one worker | 0 of 7 runs | 2026-11-01 |
| `WorkerMemoryCap` | 2 GiB | largest worker RSS 80.2 MB (squid); 26×, and exactly `MEMORY_CAP_FACTOR` (4) × `Memory` | 0 of 8,914 | 2026-11-01 |
| `ArchiveDepth` | 8 levels | set by design, measured with the Epic 3 readers before Story 5.2 | n/a | 2026-11-01 |
| `DecompressionRatio` | 100 | set by design, measured with the Epic 3 readers before Story 5.2 | n/a | 2026-11-01 |
| `TotalDecompressedBytes` | 16 GiB | set by design, measured with the Epic 3 readers before Story 5.2 | n/a | 2026-11-01 |
| `EntryMessageBytes` | 64 MiB | set by design, measured with the Epic 3 readers before Story 5.2 | n/a | 2026-11-01 |

- `Memory` was 128 MiB in the first measurement. The review found that it broke the
  `FileSize` rule (17.0 × 16 MiB = 272 MiB), so the default was raised to 512 MiB and the
  rule kept.
- The four "set by design" limits have no reader to measure against yet; the open item is
  in `_bmad-output/implementation-artifacts/deferred-work.md`.
