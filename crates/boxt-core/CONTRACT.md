# boxt-core contract

`boxt-core` owns every type that crosses a crate or process boundary. No other crate
defines a second shape for any of them. This file holds the rules the types cannot
express. The architecture spine's AD numbers and the round-2 review's `RA2-` ids name
where each rule comes from.

`boxt-core` depends on `serde` and `serde_json` only, and on no workspace crate (AD-1).

## Lists and order (AD-6)

- No type uses a hash map or hash set. A test scans `src/` for both.
- A `Vec` is positional: its order is the order of the thing it describes. A `SortedVec` is
  sorted by its element's `Ord`, which is derived, so the sort key is the element's field
  order, declared next to the element type. `SortedVec` sorts on construction and on
  deserialization.

| List | Order |
| --- | --- |
| `Location.frames` | outermost container first |
| `Fact::*.guards`, `Finding.guards` | outermost directive first |
| `Fact::Call.args`, `ShellCommand.assignments`, `.argv`, `.redirections` | as written |
| `GuardFrame.identifiers` | sorted, `Text` byte order |
| `WorkerMsg::Facts.facts`, `.Regions.regions`, `.Notes.notes`, `CompileUnits.entries` | as the reader produced them |
| `Settings.inputs` | input order as given; it is the `InputRef.index` |
| `Settings.excludes` | as given |
| `Settings.encodings`, `WorkerSettings.encodings` | as given; the first declaration that matches a path wins |
| `Settings.targets`, `.ignored_macros` | sorted |
| each `SortedVec` field of `Profile`, `Control`, `Coverage` and `Result` | sorted |
| a list inside an element of a sorted list (`Finding.guards`, `Finding.location.frames`) | its own order, from the rows above |

`Result.findings` sorts by `Finding`'s field order, `location` first, so the fixed order is
byte-exact on every host. User grouping and sort live only in `Result.groups` (Story 6.2).

Canonical order for facts and outcomes, used by the collector for run-wide cuts and for
the order it hands facts to `boxt-rules`, is `CanonicalKey`'s order: input index, then the
job's path, then the `EntryPath`, then `Location`. Every comparison is on byte forms. An
outcome's key has no location and sorts before the facts of its entry. `Fact` and
`WorkerMsg` have no `Ord`, so nothing can sort them by variant order by mistake; the
collector sorts by `CanonicalKey::for_fact` and `CanonicalKey::for_outcome`.

## Text (AD-11, RA2-L4, RA2-M3)

- Input-derived strings are `Text`: UTF-8 when valid, the raw bytes otherwise. `from_bytes`
  picks the form, so one byte sequence has one value.
- `from_wide` turns a Windows name into WTF-8: paired surrogates become UTF-8, a lone
  surrogate becomes its 3-byte generalized UTF-8 form.
- `Text` compares and sorts by its byte form.
- JSON form: a string when valid UTF-8, else `{"bytes_base64": "<standard base64, padded>"}`.
  Decoding accepts both, and a `bytes_base64` value that is valid UTF-8 becomes the string
  form.
- Nothing in `boxt-core`, the readers, the rules or verdict escapes `Text`. Each renderer
  escapes it once.

## Location and frames (AD-15, RA2-H6)

- A `Location` is the input (`index`, and `given`, the path as given), the container frames,
  the path relative to the innermost root, an optional line and a scope. It compares
  byte-exactly. Nothing in `boxt-core` folds case.
- `RelPath` is non-empty, uses `/` separators, does not start or end with `/`, and has no
  empty, `.` or `..` segment and no NUL byte. A backslash is an ordinary byte. The path of a
  single-file input is its file name.
- Only the router pushes frames. A container reader returns its frame's kind and name when
  it opens; it never pushes one.
- RPM and DEB readers decode their own payload (cpio, `data.tar.*`, `control.tar.*`) and
  yield installed paths as entries of one package frame, named from the package header
  without its version. `data.tar.xz` is never a frame.
- A tar or zip that is an input or a nested entry gets an `Archive` frame named by
  `frame_name`. It names the frame from the bytes after the last `/` only, so a caller
  passes the entry path or the file name; a Windows input path as given is split by the
  caller on its own separators first. It strips the
  longest matching suffix of `.tar.gz .tgz .tar.xz .txz .tar.bz2 .tbz2 .tar.zst .tar .zip
  .gz .xz .bz2 .zst` (exact case, so `APP.TAR.GZ` keeps its suffix), then cuts before the
  first `-`, `_` or `-v` followed by an ASCII digit. A cut that would leave nothing is not
  made. An empty name (an empty input, or one ending in `/`) gives `None`, and the caller
  names no frame from it.

## Facts (AD-2, AD-18, RA2-H5, RA2-M5)

- `Fact` is closed. Each kind declares in `FACT_KINDS` the `FileKind`s it may come from and
  its field names; every instance carries its own `file_kind` and `location`, and
  `Fact::validate` refuses a `file_kind` its kind does not allow.
- Fact kinds name structure only (calls, keys, symbols, settings, commands). A behaviour
  such as a fixed user id, forking or dynamic loading is always a rule's output.
- Dynamic loading is not a fact kind. `boxt-rules` derives `Observation::DynamicLoad` from
  one symbol list in the corpus manifest, matched only against binary `Import` facts.
- Source facts carry their full guard stack. `RegionId` is the index of a guarded region
  within its file, so the pair of the fact's file (input, frames, path) and the `RegionId`
  is unique in a run. A `GuardRegion` fact describes each guarded region once.
- `GuardFrame` reports the directive as written: its kind, the branch the region sits on,
  the raw text, the identifiers it names, and whether that branch has a non-`#error`
  alternative. Readers never classify a guard (AD-18).
- Any change to a fact kind, a field or a type a fact holds bumps `RULE_SCHEMA_VERSION`.
  The `fact_kinds` test traces every fact kind through serde and compares the result
  against `tests/snapshots/fact-kinds-v<RULE_SCHEMA_VERSION>.txt`: the table, each kind's
  fields with their declared integer widths and whether each may be null, and every
  reachable type with every serialized variant name. It holds no fixture values, and it
  fails on any difference. Every unit enum a fact holds (`FileKind`, `Directive`, `Branch`,
  `FrameKind`, `RedirectOp`), and `FactKind` and `LimitName`, is declared through the
  `closed_enum!` macro, which also generates its `ALL`; the traced fixtures are built from
  `ALL`, so a new variant reaches the snapshot without any test edit, and a test checks
  each `ALL` against serde's own variant table. `Scope` carries data, so its fixtures are
  listed by hand: an exhaustive match names each fixture's variant, and a test checks
  those names against serde's variant table for `Scope`, so a variant with no fixture
  fails. The fixtures include a raw-bytes `Text`, so the snapshot records the
  `{bytes_base64}` form. To change facts: bump the
  version, then run the tests with `BOXT_WRITE_SNAPSHOT=1`, which writes the new version's
  file and never overwrites an existing one. Recorded snapshots are never edited.

## Shell lexer (RA2-H5)

`shell::lex` is the only shell lexer. Every reader that meets a script (RPM scriptlets, DEB
maintainer scripts, SysV init scripts) uses it and emits one `Fact::ShellCommand` per
command, with the command's line in the `Location`. A script with a non-shell interpreter
is never lexed: it is `NotChecked` with `UnsupportedInterpreter`.

It does POSIX `sh` word and command splitting, plus the bash forms seen in maintainer
scripts, and expands nothing.

- Quotes are removed and `\` escapes applied. A line continuation is `\` + LF or `\` +
  CR LF. A CR right before an LF is a blank and ends a word on every path: top level,
  substitutions, backticks, here-document delimiters and body lines. Any other CR is an
  ordinary byte of its word.
- `$(…)`, `$((…))`, `${…}` and backticks stay inside their word as written, at top level
  and inside double quotes. `${…}` keeps nested quotes. Inside `$(…)`, a `#` that starts a
  word comments to the end of the line, a nested `case` is tracked (`case` and `esac`
  count only in command position, and `esac` also right after `in` or an arm end, so
  `$(echo case in)` is plain words), and a here-document's body is copied verbatim up to
  its delimiter, so none of them closes the substitution early. Inside `((…))` or
  `$((…))` arithmetic, `<<` is a shift and starts no here-document. Backticks copy
  here-document bodies the same way.
- Process substitution `<(…)` and `>(…)` is one word, copied as written like `$(…)`.
- `$'…'` decodes `\n \t \r \\ \' \" \? \a \b \e \E \f \v`, `\xHH` (one or two hex digits)
  and `\NNN` (one to three octal digits, so `\0` and `\012` work as in bash); any other
  escape stays as written. `$"…"` is a double-quoted string.
- `;`, `;;`, `;&`, `;;&`, `&&`, `||`, `|`, `|&`, `&`, `(`, `)` and newlines end a command.
  `#` at the start of a word starts a comment.
- Unquoted reserved words at the start of a command (`if then else elif fi do done while
  until ! { }`) are dropped.
- `case WORD in`: each arm's pattern list, up to its `)`, is skipped; arm bodies end at
  `;;`, `;&` or `;;&`; `esac` closes the block. `case` nests.
- `for NAME in WORDS; do`, `for NAME do`, `for ((…))` and `select`: the header is not a
  command. An arithmetic command `((…))` is not a command. A `((` whose inner `(` is not
  closed by the `)` just before the final `)` is two nested subshells, lexed as commands.
- A function definition (`name()`, `name ()`, `function name`, `function name()`) is not a
  command; its body is lexed.
- `[[ … ]]` is one command whose `argv` holds every word and operator up to `]]`. A
  newline right after `&&` or `||` inside it continues the test; any other newline ends it.
- Leading unquoted `NAME=value` and `NAME+=value` words go to `assignments`, with
  `append` set for `+=`. An array value `NAME=(…)` is copied as written into `value`, so
  its words are never commands. The first other word starts
  `argv`, and later `NAME=value` words are arguments. `argv` is empty only when
  `assignments` or `redirections` is not, and a command with none of the three is not
  emitted.
- Redirections, with any leading fd, go to `redirections`: `> >> >| < <> >& <& &> &>> <<
  <<- <<<`. Here-document bodies are skipped; `<<-` strips leading tabs before matching the
  delimiter. `<<<` is a here-string and starts no body.
- An unterminated quote, here-document or substitution, or a redirection operator with no
  target word, is `LexError{line, kind: UnterminatedQuote | UnterminatedHeredoc |
  UnterminatedSubstitution | MissingRedirectTarget}`, with the line where it began. The
  reader maps it to `NotChecked{Unparseable}` for that script and emits none of its
  commands.
- `shell::ShellCommand` carries `line`, `assignments`, `argv` and `redirections`.
  `Fact::ShellCommand` carries `assignments`, `argv` and `redirections`.

## Worker protocol (AD-3, AD-14, AD-17, RA2-C3, RA2-H4, RA2-M1, RA2-M8)

### Messages and frames

- Collector to worker: `Settings(WorkerSettings)` first, then `Job`s. `WorkerSettings` holds
  limits and declared encodings only, never corpus data.
- Worker to collector: `Facts`, `Regions`, `Notes`, `EntryDone`, `CompileUnits`, `JobDone`.
- Every message is one frame: a `u32` little-endian payload length, then the `serde_json`
  payload, written by `encode_frame` and read by `decode_frame`. Story 1.4 may change the
  payload encoding behind those two functions, but only to a self-describing one: `Text`
  and the internally tagged `Fact` deserialize through `deserialize_any`.
- The frame cap is the `EntryMessageBytes` limit: both ends pass it as `max_len`.
  `encode_frame` refuses a payload over it, and `decode_frame` refuses a length prefix over
  it before reading the payload.
- `decode_frame` returns `Ok(None)` at end of stream on a frame boundary, and
  `Ok(Some(Decoded{msg, payload_len}))` for a frame. Its errors are `Truncated`, `TooLong`,
  `Decode` and `Io`. Every `FrameError` from a worker, `Io` included, is
  `Fault(BadFrame)` for that job, never a run error. A stream that ends before `JobDone`
  is a fault named by the worker's exit (`Crash`, `Abort`, `StackOverflow`, `Exited`).
- The release profile uses `panic = "unwind"`, so the worker catches a reader panic,
  reports `Fault(Panic)` for the entry being read, and goes on. `Abort` is a process
  abort (`abort()`, `SIGABRT`), seen only from the worker's exit.
- The collector runs `Fact::validate` on every fact it receives. A fact that fails is
  `Fault(BadFrame)` for the job, as if the frame had not decoded.
- Only the parent ever stops a worker. Cancelling a job means killing its worker and
  starting a fresh one.

### Jobs

- A `Job` names its input and the job file's path relative to the input root. When the
  input is itself a file, the worker opens `input.given`; otherwise it opens `input.given`
  joined with `path`. The worker tells the two apart by opening `given` no-follow and
  checking its type.
- A `CompileDatabase` job is dispatched first and alone. The worker answers with one or
  more `CompileUnits`, holding each entry's `directory` and `file` as written and in the
  database's order, then `JobDone`. `encode_compile_units` splits the list into frames,
  each within the frame cap; the collector concatenates them in arrival order. A single
  unit over the cap makes the job `NotChecked` with `LimitExceeded(EntryMessageBytes)` on
  its `JobDone`.
- A worker runs one job at a time, on one thread (RA2-M8). Parallelism comes from the pool,
  one worker per core by default.

### Entry numbering and commit

- `entry` is the chain of entry indices from the job's file, which is `[]`.
- A container's own facts (headers, scripts) are its entry 0: `[0]` for the job's file,
  `[k, 0]` for a container nested at `[k]`. They have their own `EntryDone`, sent before any
  child.
- A worker sends entries in lexicographic `EntryPath` order. Every `Facts`, `Regions` and
  `Notes` message for an entry comes before that entry's `EntryDone`.
- An entry's facts, regions and notes count only once its `EntryDone` arrives, whatever its
  parent's state. When that `EntryDone`'s outcome is not `Read`, the collector discards what
  was sent for the entry.
- A job whose file is not a container sends its facts on `[]` and ends with
  `JobDone{outcome: Some(..)}`, which commits them. A container job ends with
  `JobDone{outcome: None}`: its own outcome went on `EntryDone{[0]}`.
- A not-checked outcome travels only on `EntryDone` or `JobDone`, never as a fact, region or
  note.
- Per entry, facts are capped by `EntryFacts` and message bytes by `EntryMessageBytes`.
  Both ends keep one `EntryBudget` per entry and charge it only through
  `EntryBudget::charge_frame(msg, payload_len)`, with every `Facts`, `Regions` and `Notes`
  frame of that entry: it adds `payload_len` (the frame's payload bytes, without the
  4-byte prefix) to the bytes, and the number of facts in a `Facts` frame to the facts.
  Other messages charge nothing. An entry over either limit is `NotChecked` with
  `LimitExceeded` naming the limit (`exceeded_outcome`), and contributes no facts.
- When `encode_frame` refuses a frame for an entry as `TooLong{len}`, the worker does not
  send it. It charges the message with `len`, which passes `EntryMessageBytes`, sends
  nothing more for the entry, and sends its `EntryDone` with `exceeded_outcome()`.

### Faults and resume

- When a worker dies, or sends a bad frame, the collector computes
  `faulting_entry(committed, last_seen)`. It takes the greater of `last_seen` and the last
  committed entry; that entry's parent is the deepest open level. The faulting entry is
  that level's last committed index plus 1, or `[.., 0]` when nothing at that level is
  committed, or `[]` when nothing is committed at all.
- `[]` makes the whole job `NotChecked` with the fault. Otherwise the faulting entry is
  `NotChecked` with the fault, and the collector re-dispatches the job once with
  `resume_after` set to it. The worker sends nothing for any entry where
  `resume_skips(entry, resume_after)` holds, entry 0 included: entries up to and including
  `resume_after`, and everything under it.
- When a worker opens an entry, before it reads any byte of it, it sends
  `WorkerMsg::opened(entry)`, which is `Facts{entry, facts: []}`, so `last_seen` names an
  entry that faults before sending anything. A container worker sends
  `opened([.., next])` before it parses the next entry header, so a fault while moving to
  the next child lands on that index: committed `[2, 1]` then `opened([2, 2])` faults
  `[2, 2]`, and the resume with `resume_after = [2, 2]` sends `[3]` on. A worker that dies
  on opening `[3]` faults `[3]`, and the resume reads `[4]` on.
- A container entry `[k]` is opened as `[k]` before its `[k, 0]`. That `opened([k])`
  needs no `EntryDone` of its own: the container's outcome travels on `[k, 0]`.
- When the container has no more entries, the header read that finds its end is still
  attributed to the next index, the container's end. A fault there marks that index
  `NotChecked`, and the retry skips straight past it to the container's next sibling.
- So the faulting entry is the one being opened or read, or the index after a container's
  last child when the fault comes at its end. The empty frame is charged like any
  `Facts` frame and commits nothing.
- A second fault in the same job makes the rest of it one
  `Aggregate{RepeatedFault, count: None}`.
- A file ended by the OS memory cap or the watchdog is retried once in a fresh worker. Its
  outcome, from the retry, is marked `host_dependent` whatever the retry gives. When the
  retry is also killed, the outcome is `NotChecked` with `LimitExceeded(Memory)` or
  `LimitExceeded(Watchdog)`; there is no other reason for either kill.

## Outcomes and counting (AD-10, RA2-H3)

- Every file and every container entry gets exactly one outcome. A container counts once,
  through its entry 0; `JobDone{None}` adds nothing.
- `Outcome.file_kind` is the `FileKind` the router identified, or `None` when no reader
  claimed the file. For a job that faulted before reporting, the collector sets it from the
  same routing decision it would make by name; a container that is not a package (tar,
  zip) has `None`.
- Reasons are a closed enum with no free text; a reason that names a limit, a fault, a
  special file type or a link target carries that closed value. `Details` holds typed
  measurements only.
- `verdict_counts` gives the verdict-line counts: files read are the `Read` outcomes; files
  not checked are the `NotChecked` outcomes plus each `Aggregate`'s count. An `Aggregate`
  with no count adds `at_least`, never below 1, and makes the total `AtLeast`.
  `OutOfScope` adds nothing: it feeds only the FR-5 excluded-path counts.
- `sm3_share` counts files whose `file_kind` is `Source` (C/C++ in release 1), except
  `OutOfScope` ones and `NotInCompileDatabase` ones, as `NotChecked` or as `Aggregate`,
  which leave both counts. The
  numerator is the files not checked for any reason plus the read files with any
  unreadable region. A zero denominator gives `None` ("n/a").

## Compile database (AD-21, RA2-H7)

`CompileUnit` carries `directory` and `file` exactly as written. The collector alone
normalizes them and matches them to walked paths. `SOURCE_EXTENSIONS` and
`HEADER_EXTENSIONS` are the one extension table, matched ignoring ASCII case. `classify`
takes the relative path, or the file name, of a file the router found to be C/C++. It
takes the part after the last `/` only (a backslash is an ordinary byte, as in `RelPath`):
a listed source extension is `Source`; anything else, unlisted or extensionless included,
is `Header`.

## Limits, settings and versions (AD-4, AD-17, AD-22)

- `LIMITS` is the one limits table: names from FR-38 plus `EntryFacts` and
  `EntryMessageBytes`, each with its flag name, scope, unit and default. The defaults are
  provisional until the readers measure them.
- Every limit is at least 1. `Limits::set`, `Limits::with` and deserialization refuse 0
  with `LimitError::Zero`, and deserialization refuses unknown fields. `Limits` is read
  through `get`; its fields are private. `EntryMessageBytes` is at most `u32::MAX`, the
  largest length the frame prefix can carry; above it is `LimitError::AboveMax`.
- `Settings.jobs` is `Option<NonZeroU32>`: `None` means one worker per core.
- `Settings` is built only by the bin. Path patterns keep their base and are never
  rewritten. A worker receives `Settings::worker()` and nothing else.
- The rule-schema, profile-format, waiver-file, project-file and JSON-output versions are
  the constants in `version`. Every loader checks against them and every report states
  them through `Versions::CURRENT`.

## Verdict types (AD-5, AD-16, AD-19, RA2-M6)

- `Tier`'s order runs worst to best, so the worst of several tiers is their minimum.
- Ids are kebab-case segments joined by `/`. Rule ids have at least two segments
  (`<dimension>/<name>`); user ids start with `user/`.
- `Finding`, `Evidence`, `Observation`, `Coverage`, `Profile`, `Control` and `Result` live
  here. Their fields are the minimum Epic 1 needs; later stories extend them.
- `boxt-verdict` depends on `boxt-core` only, never on `boxt-rules`. It parses the waiver
  file itself, with its own size and depth limits.
