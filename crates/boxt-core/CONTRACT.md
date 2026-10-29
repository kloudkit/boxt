# boxt-core contract

`boxt-core` owns every type that crosses a crate or process boundary. No other crate
defines a second shape for any of them. This file holds the rules the types cannot
express. The architecture spine's AD numbers, the round-2 review's `RA2-` ids and the
Story 1.1 spine review's `S-F` ids name where each rule comes from.

`boxt-core` depends on no workspace crate (AD-1). Its dependencies are pure-Rust crates
from the spine's Stack table, at the Stack versions, each added by the story that needs it.
Story 1.1 adds `serde` (with `derive`) and `serde_json` only; base64 for `Text` is
hand-written.

## Lists and order (AD-6)

- No type uses a hash map or hash set. A test scans `src/` for both.
- A `Vec` is positional: its order is the order of the thing it describes. A `SortedVec` is
  sorted by its element's `Ord`, which is derived, so the sort key is the element's field
  order, declared next to the element type. `SortedVec` sorts on construction and on
  deserialization.

| List | Order |
| --- | --- |
| `Location.frames`, `WorkerMsg::Opened.frames` | outermost container first |
| `Fact::*.guards`, `Finding.guards`, `Observation::WindowsBranch.guards` | one `GuardChain` per enclosing conditional chain, outermost first |
| `GuardChain.frames` | as written: the chain's opening directive first, the branch holding the code last |
| `Fact::Call.args`, `ShellCommand.assignments`, `.argv`, `.redirections` | as written |
| `GuardFrame.identifiers` | sorted, `Text` byte order |
| `WorkerMsg::Facts.facts`, `.Regions.regions`, `.Notes.notes`, `CompileUnits.entries` | as the reader produced them |
| `CollectedFile.regions`, `.notes` | as received |
| a run's `CollectedFile`s | sorted by `CollectedFile.key`, a `CanonicalKey` |
| `Settings.inputs` | input order as given; it is the `InputRef.index` |
| `Settings.excludes` | as given |
| `Settings.encodings`, `WorkerSettings.encodings` | as given; the first declaration that matches a path wins |
| `Settings.targets`, `.ignored_macros` | sorted |
| `RuleCoverage.file_kinds` | sorted alternatives, each a sorted set of `FileKind`s |
| each `SortedVec` field of `Profile`, `Control`, `Coverage` and `Result` | sorted |
| a list inside an element of a sorted list (`Finding.guards`, `Finding.location.frames`) | its own order, from the rows above |

`Result.findings` sorts by `Finding`'s field order, `location` first, so the fixed order is
byte-exact on every host. User grouping and sort live only in `Result.groups` (Story 6.2).

Canonical order for facts and outcomes, used by the collector for run-wide cuts and for
the order it hands facts to `boxt-rules`, is `CanonicalKey`'s order: input index, then the
job's path, then the `EntryPath`, then `Location`. Every comparison is on byte forms. An
outcome's key has no location and sorts before the facts of its entry. `Fact` and
`WorkerMsg` have no `Ord`, so nothing can sort them by variant order by mistake; the
collector sorts by `CanonicalKey::for_fact` and `CanonicalKey::for_outcome`. Both return
`None` for a compile-database job, whose output never joins the run's facts or outcomes.

## Wire form (AD-14, AD-5)

Every type that crosses a process boundary, or is serialized into a report, uses a serde
form that any serde format can carry: enums are externally tagged (serde's default; no
`#[serde(tag)]`, `untagged` or `flatten`), and nothing calls `deserialize_any` unless the
format is human-readable. A test round-trips every collector and worker message through a
format that is not self-describing and refuses `deserialize_any`. Story 1.4 may choose any
payload encoding behind `encode_frame` and `decode_frame`.

## Text (AD-11, RA2-L4, RA2-M3)

- Input-derived strings are `Text`: UTF-8 when valid, the raw bytes otherwise. `from_bytes`
  picks the form, so one byte sequence has one value.
- `from_wide` turns a Windows name into WTF-8: paired surrogates become UTF-8, a lone
  surrogate becomes its 3-byte generalized UTF-8 form.
- `Text` compares and sorts by its byte form.
- In a human-readable format (JSON, YAML): a string when valid UTF-8, else
  `{"bytes_base64": "<standard base64, padded>"}`. Decoding accepts both, and a
  `bytes_base64` value that is valid UTF-8 becomes the string form.
- In any other format: the byte form, through `serialize_bytes`, read back through
  `deserialize_byte_buf` and `from_bytes`.
- Nothing in `boxt-core`, the readers, the rules or verdict escapes `Text`. Each renderer
  escapes it once.

## Location and frames (AD-15, RA2-H6)

- A `Location` is the input (`index`, and `given`, the path as given), the container frames,
  the path relative to the innermost root, an optional line and a scope. It compares
  byte-exactly. Nothing in `boxt-core` folds case.
- `path` is `None` when the location is the innermost frame itself: a package's own facts,
  and its install scripts. A location with no frames always has a path.
- Lines are 1-based. No location has line 0.
- `RelPath` is non-empty, uses `/` separators, does not start or end with `/`, and has no
  empty, `.` or `..` segment and no NUL byte. A backslash is an ordinary byte. The path of a
  single-file input is its file name.
- `entry_path(raw)` is the one place an archive, cpio or package entry name becomes a
  `RelPath`. It drops empty and `.` segments, so a leading `/` or `./` and a trailing `/` go,
  and keeps the bytes of every other segment. A `..` segment, a NUL byte or a name with no
  segment left is an `EntryNameError`. The router gives that entry
  `NotChecked{UnsafeEntryName}` with `Details::EntryName` holding the raw name, at the
  location of its container (path `None`), and reads nothing from it.
- Only the router pushes frames. A container reader gives its frame's name after it opens;
  it never pushes one.
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
- Install scripts (RPM scriptlets, DEB maintainer scripts) are entries of their package
  frame: path `None`, scope `Scope::Script(ScriptName)`. `SCRIPT_NAMES` is the one table
  from each format's native name (`%post`, `postinst`) to its `ScriptName`, so the same
  script in an RPM and a DEB has the same scope.
- `PathText` is a path-typed fact value, always held normalized by `normalize_value_path`:
  empty and `.` segments dropped, `..` resolved against the segment before it (dropped at
  the root of an absolute path, kept at the start of a relative one), no trailing `/`. An
  empty value stays empty; a relative value that resolves to nothing is `.`. Deserializing
  normalizes too. Rules match path tests against `PathText` fields only.

## Facts (AD-2, AD-18, RA2-H5, RA2-M5)

- `Fact` is closed. Each kind declares in `FACT_KINDS` the `FileKind`s it may come from, the
  scopes its location may have, its `PathRule`, and its field names. Every instance carries
  its own `file_kind` and `location`.
- The collector runs `Fact::validate` on every fact it receives. It refuses a `file_kind`,
  a scope or a path the kind's row does not allow, line 0, a malformed guard chain, and a
  `package_script` whose `name` differs from its scope. A fact that fails is
  `Fault(BadFrame)` for the job, as if the frame had not decoded.
- Fact kinds name structure only (calls, keys, symbols, settings, commands). A behaviour
  such as a fixed user id, forking or dynamic loading is always a rule's output.
- Dynamic loading is not a fact kind. `boxt-rules` derives `Observation::DynamicLoad` from
  one symbol list in the corpus manifest, matched only against binary `Import` facts.

### Per-kind rules (S-F8)

`PathRule::File`: `path` is the file's path in its innermost root. `Frame`: `path` is
`None`. `FrameForScripts`: `None` with scope `Script`, a path with any other scope.

| Kind | From | Scope | Path | Line | Value forms |
| --- | --- | --- | --- | --- | --- |
| `call` | source | `Function{qualified_name, signature}` inside a function, else `File` | file | the call's first line | `callee` as written, qualification included; `args` each argument's text as written |
| `include` | source | as `call` | file | the directive's line | `target` without its delimiters; `system` is true for `<…>` |
| `string_literal` | source | as `call` | file | the literal's first line | the literal's bytes with C escapes decoded, prefix and quotes dropped; adjacent literals stay separate facts |
| `string_literal` | binary | `File` | file | none | the bytes as found, without the terminating NUL |
| `config_key` | config | `ConfigKey(key)` | file | the key's line | `key` is the full key, nested names joined with `.` (INI: `section.key`); `value` is the scalar as written after unquoting, `None` for a non-scalar |
| `guard_region` | source | as `call`, at the branch's directive | file | the branch's directive line | `end_line` is the line of the directive that ends the branch |
| `import`, `export` | binary | `Symbol(symbol)` | file | none | `symbol` as in the dynamic symbol table, not demangled; `version` the symbol version name (`GLIBC_2.34`), `None` when unversioned |
| `needed_library` | binary | `File` | file | none | `name` as in `DT_NEEDED` |
| `package_file` | package | `File` | frame | none | `path` the installed path as a `PathText`; `mode` the full `st_mode` (type and permission bits); `user`, `group` names from the header |
| `package_dependency` | package | `File` | frame | none | `name` as in the header; `constraint` operator and version as written (`>= 2.17`) |
| `package_script` | package | `Script(name)` | frame | none | one per install script, on the package's entry 0; `interpreter` as written (`-p <interp>` for RPM), `None` when the format's default applies |
| `service_setting` | service | `UnitSection{section, key}` | file | the key's line | `section` without brackets, `key` as written, `value` after `=` with continuations joined and outer blanks trimmed |
| `shell_command` | package | `Script(name)` | frame | the command's line in the script | from `shell::lex` |
| `shell_command` | service | `File` | file | the command's line | from `shell::lex`, for a SysV init script |

- A script with a body is also an entry of the package (below), holding its `shell_command`
  facts. A script with no body (`%post -p /sbin/ldconfig`) is only its `package_script`
  fact.
- `config_key` comes only from config files. A key-like string in C/C++ source is a
  `string_literal`.

### Guards (AD-18, S-F7)

- A source fact's `guards` has one `GuardChain` per enclosing conditional chain, outermost
  first. A chain's `frames` run from its opening `#if`, `#ifdef` or `#ifndef` through every
  `#elif`, `#elifdef`, `#elifndef` and `#else` up to and including the directive whose
  branch holds the code. Each frame holds the directive, its text as written, the
  identifiers it names, and `holds_code`, which is true on the chain's last frame only.
  `GuardChain::validate` checks that shape.
- So `#ifdef B` nested in the `#else` of `#if A` is two chains: `[#if A, #else*]` then
  `[#ifdef B*]`, where `*` marks `holds_code`.
- `has_alternative` belongs to the chain: true when another branch of that chain holds a
  token other than whitespace, a comment or `#error`.
- A chain's `region` is the `RegionId` of the branch holding the code: the index of that
  branch within its file, so the pair of the fact's file (input, frames, path) and the
  `RegionId` is unique in a run. A `GuardRegion` fact describes each branch once.
- Readers never classify a guard (AD-18).

### The rule-schema snapshot (AD-2)

- Any change to a fact kind, a field, a table column or a type a fact holds bumps
  `RULE_SCHEMA_VERSION`: the minor for an added kind or field, the major for anything else.
  The `fact_kinds` test traces every fact kind through serde and compares the result
  against `tests/snapshots/fact-kinds-v<RULE_SCHEMA_VERSION>.txt`: the table (file kinds,
  scopes, path rule, fields), each kind's fields with their declared integer widths and
  whether each may be null, every reachable type with every serialized variant name, and
  each named newtype (`PathText`). It holds no fixture values, and it fails on any
  difference.
- Every unit enum a fact holds (`FileKind`, `Directive`, `FrameKind`, `RedirectOp`,
  `ScriptName`), and `FactKind`, `ScopeKind`, `PathRule` and `LimitName`, is declared
  through the `closed_enum!` macro, which also generates its `ALL`; the traced fixtures are
  built from `ALL`, so a new variant reaches the snapshot without any test edit, and a test
  checks each `ALL` against serde's own variant table. `Scope` carries data: its fixtures
  come from an exhaustive match on `ScopeKind`, and a test checks `ScopeKind`'s names
  against serde's variant table for `Scope`. The fixtures include a raw-bytes `Text`, so the
  snapshot records the `{bytes_base64}` form.
- To change facts: bump the version, then run the tests with `BOXT_WRITE_SNAPSHOT=1`, which
  writes the new version's file and never overwrites an existing one. Recorded snapshots
  are never edited. Version 1.0 was re-recorded once, before anything shipped.

## Shell lexer (RA2-H5)

`shell::lex` is the only shell lexer. Everything that meets a script uses it: the router for
the install-script entries of an RPM or DEB frame, and the service reader for SysV init
scripts. Each command becomes one `Fact::ShellCommand`, with the command's line in the
`Location`. `is_shell_interpreter` decides whether a script is shell: no interpreter (the
RPM default), or a first word, or the program after `env` and its options, whose file name
is in `SHELL_INTERPRETERS` (`sh bash dash ash ksh mksh`). A script with any other
interpreter is never lexed: it is `NotChecked` with `UnsupportedInterpreter` and
`Details::Interpreter`.

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

## Reader boundary (AD-14, S-F1)

`read` holds the one interface between the router and every reader. Readers depend only on
`boxt-core` and never on the router, so these traits are the whole contract.

- Readers get only the bytes they are handed. They never open, stat or name a file on the
  host, and never see another file.
- `Reader` is a leaf reader (C/C++, ELF, service, config): `read(bytes, ctx, sink)` gets the
  whole file, already bounded by `FileSize`, and returns `Ok(())` for `Read` or a
  `NotRead{reason, details}`.
- `ContainerReader` is a container reader (RPM, DEB, tar, zip). `open(input, ctx)` reads
  the header and returns a `Container`; it emits no fact, since its frame is not pushed
  yet. The router then takes `Container::name()` (the package name from the header, or
  `None` for an archive, which the router names with `frame_name`), pushes the frame, sends
  `Opened` for entry 0 and calls `own_facts`, then calls `next_entry` until it returns
  `None`.
- An `Entry` has a raw `EntryName` (a path, or an install script with its interpreter), an
  `EntryKind` and the entry's bytes as an `Input`: a stream, or a seekable reader where the
  container can give one (a zip on a seekable outer input, or one buffered in memory). The
  router normalizes a path with `entry_path`, gives no index to a `Directory`, and gives a
  `Link` `NotChecked{Symlink(Unresolved)}` and a `Special` entry `NotChecked{SpecialFile}`;
  neither is followed or read.
- `ReadCtx` holds the base `Location` of the file being read (input, frames, path; line
  `None`, scope `File`, or the script's scope), the `WorkerSettings`, and one `WorkBudget`.
  Readers build each fact's location with `ctx.at(line, scope)`.
- `WorkBudget` counts against `Limits`: `work` steps (`Work`; tree-sitter steps through its
  progress callback), `buffer` and `release` for bytes held in memory (`Memory`, the counted
  per-file buffer), and depth. Each file and entry gets a fresh budget for work and bytes; a
  container's entries get `nested()`, one level deeper, which fails past `ArchiveDepth`. A
  value over its limit is `Exceeded{limit, value, max}`, which becomes
  `NotChecked{LimitExceeded(limit), Measured{value, limit: max}}`.
- `FactSink` takes facts, regions and notes. The router's sink charges the entry's
  `EntryBudget`; when it refuses, the reader stops and returns the `Exceeded` it got. A
  reader that stops for any reason returns `NotRead`, and the router discards what the
  entry sent.

## Worker protocol (AD-3, AD-14, AD-17, RA2-C3, RA2-H4, RA2-M1, RA2-M8)

### Messages and frames

- Collector to worker: `Settings(WorkerSettings)` first, then `Job`s. `WorkerSettings` holds
  limits and declared encodings only, never corpus data.
- Worker to collector: `Opened`, `Facts`, `Regions`, `Notes`, `EntryDone`, `CompileUnits`,
  `JobDone`.
- Every message is one frame: a `u32` little-endian payload length, then the payload,
  written by `encode_frame` and read by `decode_frame`. The payload is `serde_json` until
  Story 1.4 chooses an encoding; any serde format works (Wire form).
- The frame cap is the `EntryMessageBytes` limit: both ends pass it as `max_len`.
  `encode_frame` refuses a payload over it, and `decode_frame` refuses a length prefix over
  it before reading the payload.
- `decode_frame` returns `Ok(None)` at end of stream on a frame boundary, and
  `Ok(Some(Decoded{msg, payload_len}))` for a frame. Its errors are `Truncated`, `TooLong`,
  `Decode` and `Io`. Every `FrameError` from a worker, `Io` included, is
  `Fault(BadFrame)` for that job, never a run error.
- The release profile uses `panic = "unwind"`, so the worker catches a reader panic,
  reports `Fault(Panic)` for the entry being read, and goes on.
- Only the parent ever stops a worker. Cancelling a job means killing its worker and
  starting a fresh one.

### Jobs

- A `Job` names its input and its file. A `Route` job's file is `JobFile::InInput`, the
  path relative to the input root: when the input is itself a file, the worker opens
  `input.given`; otherwise it opens `input.given` joined with the path. The worker tells the
  two apart by opening `given` no-follow and checking its type. A `CompileDatabase` job's
  file is `JobFile::Setting`, the path as the bin resolved it from the setting, which may
  lie outside every input. `Job::is_well_formed` checks the pairing, and that only a
  `Route` job resumes.
- A worker runs one job at a time, on one thread (RA2-M8). Parallelism comes from the pool,
  one worker per core by default.

### Entry numbering and commit

- `entry` is the chain of entry indices from the job's file, which is `[]`.
- A container's own facts (header, `package_script`, `package_file`, `package_dependency`)
  are its entry 0: `[0]` for the job's file, `[k, 0]` for a container nested at `[k]`. They
  have their own `EntryDone`, sent before any child. Install scripts with a body are later
  entries of the same container.
- `Opened{entry, frames, path, scope, file_kind}` names an entry once its header is parsed:
  the frames down to the entry's innermost root, its path there (`None` for the frame
  itself and its scripts), its scope (`File`, or `Script(name)` for a script) and the
  `FileKind` the router identified (`None` when no reader claims it, and for an archive).
  Every entry that gets an outcome is opened first, and so is the job's file, as `[]`.
  `opened_location` turns it into the entry's `Location`. A nested container `[k]` is
  opened as `[k]` (its path in its parent) and then as `[k, 0]` (the new frame, path
  `None`); `[k]` has no `EntryDone` of its own, since the container's outcome travels on
  `[k, 0]`.
- A worker sends entries in lexicographic `EntryPath` order. Every `Facts`, `Regions` and
  `Notes` message for an entry comes after its `Opened` and before its `EntryDone`.
- `EntryDone{entry, outcome, bytes}` commits the entry. `bytes` is the decompressed bytes of
  the entry's own content, not counting its children; the collector sums them in canonical
  order for `TotalDecompressedBytes`.
- An entry's facts, regions and notes count only once its `EntryDone` arrives, whatever its
  parent's state. When that `EntryDone`'s outcome is not `Read`, the collector discards what
  was sent for the entry.
- A job whose file is not a container sends its facts on `[]` and ends with
  `JobDone{outcome: Some(..)}`, which commits them. A container job ends with
  `JobDone{outcome: None}`: its own outcome went on `EntryDone{[0]}`.
- A not-checked outcome travels only on `EntryDone` or `JobDone`, never as a fact, region or
  note.
- A container stopped by a limit after some children (an entry count, or a job's share of a
  run-wide limit) sends `EntryDone{[.., n], Aggregate{…}}` at its next index `n`, never 0,
  and nothing more for that container. `ends_container` names the container such a message
  ends. That index is never opened, and the aggregate takes the location of the
  container's entry 0.
- Per entry, facts are capped by `EntryFacts` and message bytes by `EntryMessageBytes`.
  Both ends keep one `EntryBudget` per entry and charge it only through
  `EntryBudget::charge_frame(msg, payload_len)`, with every `Facts`, `Regions` and `Notes`
  frame of that entry: it adds `payload_len` (the frame's payload bytes, without the
  4-byte prefix) to the bytes, and the number of facts in a `Facts` frame to the facts.
  Other messages, `Opened` included, charge nothing. An entry over either limit is
  `NotChecked` with `LimitExceeded` naming the limit (`exceeded_outcome`), and contributes
  no facts.
- When `encode_frame` refuses a frame for an entry as `TooLong{len}`, the worker does not
  send it. It charges the message with `len`, which passes `EntryMessageBytes`, sends
  nothing more for the entry, and sends its `EntryDone` with `exceeded_outcome()`.

### Faults and resume

- When a worker dies, or sends a bad frame or a fact that fails `Fact::validate`, the
  collector first attributes the fault with `faulting_entry(committed, last_seen)`, where
  `last_seen` is the last entry any message named. It takes the greater of `last_seen` and
  the last committed entry; that entry's parent is the deepest open level. The attributed
  entry is that level's last committed index plus 1, or `[.., 0]` when nothing at that level
  is committed, or `[]` when nothing is committed at all.
- `fault_scope(attributed, opened)`, where `opened` says whether an `Opened` arrived for the
  attributed entry, then decides what the fault costs:
  - `[]` or `[0]`: `Job`. The whole job is `NotChecked` with the fault at `[]`, and it is
    not re-dispatched.
  - `[.., k, 0]`: `Container([.., k])`. The container is `NotChecked` at `[.., k, 0]`, and
    the job resumes after `[.., k]`, so its header is never parsed again.
  - `[.., n]`, `n > 0`, not opened: a header fault, `RestOfContainer`. The rest of the
    container becomes one `Aggregate{reason, count: None, at_least: 1}` at `[.., n]`, and
    the job resumes after the container; when the container is the job's file there is
    nothing left to resume. Children committed before it stand.
  - `[.., n]`, `n > 0`, opened: `Entry`. Only that entry is `NotChecked`, and the job
    resumes after it.
- A re-dispatched job carries `resume_after` from `FaultScope::resume_after`. The worker
  sends nothing for any entry where `resume_skips(entry, resume_after)` holds, entry 0
  included: entries up to and including `resume_after`, and everything under it.
- A worker that dies while moving from a container's last child to the next header cannot
  be told from one that dies on the parent's next header. The fault is attributed to the
  container's next index, costs the rest of that container (nothing), and the retry starts
  at the parent's next entry; if that header faults again, the second-fault rule below
  ends the job. That is coarse, but no entry is read twice or skipped unreported.
- A second fault in the same job makes the rest of it one
  `Aggregate{RepeatedFault, count: None}` at the second fault's attributed entry.
- The fault's reason comes from `classify_exit(Platform, RawExit)`, or is
  `Fault(BadFrame)` for a bad frame or fact. `FaultKind` holds only what every OS can tell:
  `Crash` (a signal on Unix, an NTSTATUS error code on Windows: segfault, abort and stack
  overflow all look alike), `Panic` (exit code 101, a panic that escaped the worker's own
  catch), `BadFrame` and `Exited` (any other exit before `JobDone`, 0 included).
- The worker's allocation-failure path exits with `WORKER_MEMORY_CAP_EXIT`, a code reserved
  for it. That exit is `ExitClass::WorkerMemoryCap`: the outcome is
  `LimitExceeded(WorkerMemoryCap)`, marked `host_dependent`.
- A file ended by the OS memory cap or the watchdog is retried once in a fresh worker. Its
  outcome, from the retry, is marked `host_dependent` whatever the retry gives. When the
  retry is also ended, the outcome is `LimitExceeded(WorkerMemoryCap)` or
  `LimitExceeded(Watchdog)`. When the run-wide watchdog fires, the files not yet read become
  one `Aggregate{LimitExceeded(RunWatchdog)}`, marked `host_dependent`.

## Outcomes and counting (AD-10, RA2-H3)

- Every file and every container entry gets exactly one outcome. A container counts once,
  through its entry 0; `JobDone{None}` adds nothing.
- `Outcome.file_kind` is the `FileKind` the router identified, from the entry's `Opened`,
  or `None` when no reader claimed the file. For a job that faulted before `Opened{[]}`,
  the collector sets it from the same routing decision it would make by name, and builds
  the location from the job (no frames, the job's path); a container that is not a package
  (tar, zip) has `None`.
- `CollectedFile{key, location, outcome, regions, notes}` carries each committed outcome
  from the collector, through the bin, to verdict: `key` is `CanonicalKey::for_outcome`,
  `location` comes from the entry's `Opened` (for an aggregate at an index never opened,
  from its container's entry 0), and `regions` and `notes` are what the entry sent. A run's
  files are a `SortedVec<CollectedFile>`, in canonical order. A compile-database job never
  yields one.
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
  unreadable region (`CollectedFile::has_unreadable_region`). A zero denominator gives
  `None` ("n/a").

## Compile database (AD-21, RA2-H7, S-F10)

- `Settings.compile_database` is a `CompileDatabaseSetting{file, tree}`: the database's path
  as the bin resolved it, and the index of the input whose source tree it limits.
  `Settings::compile_database_job` builds its job. The collector dispatches it first and
  alone, before walking that tree.
- The worker answers with one or more `CompileUnits`, holding each entry's `directory` and
  `file` as written and in the database's order, then `JobDone{Some(outcome)}`.
  `encode_compile_units` splits the list into frames, each within the frame cap; the
  collector concatenates them in arrival order. A single unit over the cap makes the job
  `NotChecked` with `LimitExceeded(EntryMessageBytes)`.
- That job's outcome never joins the run's outcomes and never counts on a verdict line.
  `CompileDatabaseError::check` turns it into the run's result: `Read` is fine, anything
  else (`NotChecked`, an aggregate, a missing outcome, which is `Fault(BadFrame)`) is the
  typed run error `NotRead(reason)`, and a database that matches no walked file is
  `MatchesNoFile`. The bin maps both to exit 2 (FR-4).
- The collector alone normalizes the units and matches them to walked paths.
  `SOURCE_EXTENSIONS` and `HEADER_EXTENSIONS` are the one extension table, matched ignoring
  ASCII case. `classify` takes the relative path, or the file name, of a file the router
  found to be C/C++. It takes the part after the last `/` only (a backslash is an ordinary
  byte, as in `RelPath`): a listed source extension is `Source`; anything else, unlisted or
  extensionless included, is `Header`.

## Limits, settings and versions (AD-4, AD-17, AD-22)

- `LIMITS` is the one limits table: names from FR-38, with its memory limit and watchdog
  each split in two (below), plus `EntryFacts` and `EntryMessageBytes`, each with its flag
  name, scope, unit and default. The defaults are
  provisional until the readers measure them.
- `Memory` is the per-file buffer the readers count through `WorkBudget`; it is
  deterministic. `WorkerMemoryCap` is the OS cap each worker sets on itself before its
  first job (Job Object or `setrlimit`), a last resort whose hit is host-dependent.
  `Watchdog` is the per-file wall-clock last resort and `RunWatchdog` the run-wide one.
- `WorkerMemoryCap` is at least `MEMORY_CAP_FACTOR` (4) times `Memory`, so a file within
  its counted buffer cannot reach the OS cap through that buffer alone. `Limits::set` and
  deserialization refuse anything else with `LimitError::CapBelowMemory`, and a refused
  `set` leaves the limits unchanged; to raise both, raise the cap first.
- Every limit is at least 1. `Limits::set`, `Limits::with` and deserialization refuse 0
  with `LimitError::Zero`, and deserialization refuses unknown fields. `Limits` is read
  through `get`; its fields are private. `EntryMessageBytes` is at most `u32::MAX`, the
  largest length the frame prefix can carry; above it is `LimitError::AboveMax`.
- `Settings.jobs` is `Option<NonZeroU32>`: `None` means one worker per core.
- `Settings` is built only by the bin. Path patterns keep their base and are never
  rewritten. A worker receives `Settings::worker()` and nothing else.
- The rule-schema, profile-format, waiver-file, project-file and JSON-output versions are
  the `SchemaVersion{major, minor}` constants in `version`, printed and parsed as
  `major.minor`. An added field or kind bumps the minor; anything else bumps the major.
  A loader at version `v` accepts a file at version `f` when `v.accepts(f)`, which holds
  for any minor of the same major. Every loader checks against them and every report
  states them through `Versions::CURRENT`. Story 6.8 owns the published policy.

## Verdict types (AD-5, AD-16, AD-19, RA2-M6)

- `Tier`'s order runs worst to best, so the worst of several tiers is their minimum.
- Ids are kebab-case segments joined by `/`. Rule ids have at least two segments
  (`<dimension>/<name>`); user ids start with `user/`.
- `Finding`, `Evidence`, `Observation`, `Coverage`, `Profile`, `Control` and `Result` live
  here. Their fields are minimal; later stories extend them.
- `RuleCoverage.file_kinds` is a sorted set of alternatives, each a sorted set of
  `FileKind`s that must all be read for the rule to fire. `can_fire(read)` is true when
  every kind of some alternative was read. A rule matching `all` of a source fact and a
  package fact has the one alternative `[source, package]`; one matching `any` of an
  import or a call has `[[source], [binary]]`.
- `boxt-verdict` depends on `boxt-core` only, never on `boxt-rules`. It parses the waiver
  file itself, with its own size and depth limits.
