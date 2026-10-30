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
  deserialization. It is a sorted list: it keeps duplicates, unless the type holding it
  refuses them (`RuleCoverage`).

| List | Order |
| --- | --- |
| `Location.frames`, `WorkerMsg::Opened.frames` | outermost container first |
| `Fact::*.guards`, `Finding.guards`, `Observation::WindowsBranch.guards` | one `GuardChain` per enclosing conditional chain, outermost first |
| `GuardChain.frames` | as written: the chain's opening directive first, the branch holding the code last |
| `Fact::Call.args`, `ShellCommand.assignments`, `.argv`, `.redirections` | as written |
| `GuardFrame.identifiers` | sorted, `Text` byte order |
| `WorkerMsg::Facts.facts`, `.Regions.regions`, `.Notes.notes`, `CompileUnits.entries`, `Definitions.definitions` | as the reader produced them |
| `MacroParams.names` | as written |
| `MacroTable`, `CollectorMsg::Macros.definitions` | sorted by `MacroDef`'s derived `Ord`: name bytes, then `location`, then `params` and `body`; duplicates kept |
| `CollectedFile.regions`, `.notes` | as received |
| `Sm3.uncounted`, `.incomplete` | sorted `CanonicalKey`s: canonical order |
| a run's `CollectedFile`s | sorted by `CollectedFile.key`, a `CanonicalKey` |
| `Settings.inputs` | input order as given; it is the `InputRef.index` |
| `Settings.excludes` | as given |
| `Settings.encodings`, `WorkerSettings.encodings` | as given; the first declaration that matches a path wins |
| `Settings.targets`, `.ignored_macros` | sorted |
| `RuleCoverage.file_kinds` | sorted alternatives, each a sorted list of `FileKind`s; `RuleCoverage` refuses an empty alternative and any duplicate |
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
`None` for any job but a `Route` job: a compile-database or definitions job's output never
joins the run's facts or outcomes.

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

- `Fact` is closed. Each kind declares in `FACT_KINDS` its field names and one
  `SourceRule` row per `FileKind` it may come from: the scopes its location may have, its
  `PathRule` and its `LineRule`. These rows are the table below. Every instance carries its
  own `file_kind` and `location`.
- The collector runs `Fact::validate` on every fact it receives. It takes the row for the
  fact's `file_kind`, and refuses a `file_kind` with no row, a scope, path or line the row
  does not allow, line 0, a `Frame` fact whose location has no frame, a scope whose text
  differs from the fact's own fields (`Symbol` and `symbol` for `import` and `export`,
  `ConfigKey` and `key`, `UnitSection` and `section`, `key`), a `package_script` whose
  `name` differs from its scope, a `Script` scope whose `ScriptName` has no native name for
  the innermost frame's kind (an RPM or DEB frame), a `guard_region` with no guards, with
  `end_line` before its line, or whose `region` is not its last chain's, and a malformed
  guard chain. A fact that fails is `Fault(BadFrame)` for the job, as if the frame had not
  decoded.
- Fact kinds name structure only (calls, keys, symbols, settings, commands). A behaviour
  such as a fixed user id, forking or dynamic loading is always a rule's output.
- Dynamic loading is not a fact kind. `boxt-rules` derives `Observation::DynamicLoad` from
  one symbol list in the corpus manifest, matched only against binary `Import` facts.

### Per-kind rules (S-F8)

Each row is one `SourceRule`. Path `file` is `PathRule::File`: `path` is the file's path in
its innermost root. Path `frame` is `PathRule::Frame`: `path` is `None` and the location has
at least one frame. Line `none` is `LineRule::Absent`; any other line entry is
`LineRule::Required`.

| Kind | From | Scope | Path | Line | Value forms |
| --- | --- | --- | --- | --- | --- |
| `call` | source | `Function{qualified_name, signature}` inside a function, else `File` | file | the call's first line | `callee` as written, qualification included; `args` each argument's text as written, line ends as LF |
| `include` | source | as `call` | file | the directive's line | `target` without its delimiters; `system` is true for `<…>` |
| `string_literal` | source | as `call` | file | the literal's first line | the literal's bytes with C escapes decoded, prefix and quotes dropped; adjacent literals stay separate facts |
| `string_literal` | binary | `File` | file | none | each NUL-terminated run of at least 4 printable bytes (ASCII 0x20 to 0x7E) from the `.rodata` and `.data` sections only, without the NUL |
| `config_key` | config | `ConfigKey(key)` | file | the key's line | `key` is the key's path, its segments joined with `.` (INI: `section.key`); built only by `config_key_path`: a name segment that is empty, starts with `@`, or holds `.`, `[` or a control character (CR included) is written `["…"]` with JSON string escaping, an array element is `[n]` (0-based), and an XML attribute is `@name` (quoted as `["@…"]` by the same rule); `[n]` and `["…"]` attach to the segment before them with no dot, and a bare segment after any segment is preceded by `.` (`a[0].b`, `a["x.y"].c`); `value` is the scalar as written after unquoting, `None` for a non-scalar |
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
- `Scope::Function`: `qualified_name` is the function's name as written, namespaces and
  classes included, with the whitespace around `::` removed; `signature` is the parameter
  types only, comments and parameter names dropped, each run of whitespace (line ends
  included) collapsed to one space and none kept at either end. So the same function has
  the same scope whatever its layout.
- CR LF: every text reader treats CR LF as one line end, as LF is. No fact value, `args`
  item or scope text holds the CR of a CR LF, so a checkout with CRLF line ends gives the
  same facts and finding ids as one with LF (AD-7, FR-36). A CR that is part of the input's
  data and not a line end (in an ELF symbol name, or an escaped `\r` in a JSON or YAML
  value) is kept as data; a config-key segment holding one is quoted as above.
  `Fact::validate` refuses a CR only in `Scope::Function` text, as
  `FactError::CarriageReturn`: there it can only be a leaked line end.
- C/C++ identification: the router identifies a C/C++ file by a listed extension, plus a
  content check that works on bytes in any encoding (or runs after the core decode
  function), so a UTF-16 or code-page file still counts. A file with a listed extension that
  then fails decoding is `file_kind: Source`, `NotChecked` with the FR-10 reason that
  `boxt-read-cpp` returns (`EncodingUndeclared` or `InvalidForDeclaredEncoding`), and stays
  in SM-3; it is never `NoReader`.

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
  against `tests/snapshots/fact-kinds-v<RULE_SCHEMA_VERSION>.txt`: the table (one row per
  file kind with its scopes, path rule and line rule, then the fields), each kind's fields with their declared integer widths and
  whether each may be null, every reachable type with every serialized variant name, and
  each named newtype (`PathText`). It holds no fixture values, and it fails on any
  difference.
- Every unit enum a fact holds (`FileKind`, `Directive`, `FrameKind`, `RedirectOp`,
  `ScriptName`), and `FactKind`, `ScopeKind`, `PathRule`, `LineRule` and `LimitName`, is declared
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
the install-script entries of an RPM or DEB frame (it lexes them itself, with no reader),
and the service reader for SysV init scripts. Each command becomes one `Fact::ShellCommand`,
with the command's line in the `Location`. It splits words and commands as POSIX `sh` plus
the bash forms seen in maintainer scripts, and expands nothing. The grammar it covers is
fixed by the tests in `src/shell.rs` and `tests/matrix.rs`; the rules other crates rely on
are:

- CR LF is a line end everywhere, so no word or line number differs between a CRLF and an
  LF checkout. A line continuation is `\` + LF or `\` + CR LF.
- Leading `NAME=value` and `NAME+=value` words are `assignments`; the first other word
  starts `argv`; redirections, with any leading fd, are `redirections`. A command with none
  of the three is not emitted. `shell::ShellCommand` also carries the command's `line`.
- A `LexError{line, kind}` (an unterminated quote, here-document or substitution, or a
  redirection with no target) makes the script `NotChecked{Unparseable}`, and none of its
  commands are emitted.
- `is_shell_interpreter` decides whether a script is shell. It drops one trailing CR, then
  leading blanks and a leading `#!` and the blanks after it. No interpreter, or a blank one,
  is the format's default shell. Otherwise the program is the first word, or for `env` the
  first word after its options and `NAME=value` words: `-u`/`--unset` and `-C`/`--chdir`
  take the next word as their argument, and `-S`/`--split-string` (or `-S<word>`) splits its
  argument into the words that follow. The script is shell when the program's file name is
  in `SHELL_INTERPRETERS` (`sh bash dash ash ksh mksh`). A script with any other interpreter
  is never lexed: it is `NotChecked` with `UnsupportedInterpreter` and
  `Details::Interpreter`.
- The `interpreter` a package reader reports: for DEB, the script's shebang line after `#!`
  with leading blanks dropped and a trailing CR dropped, or `None` when the script has no
  shebang; for RPM, the scriptlet's `*PROG` tag as written, or `None` when the tag is
  absent or is `/bin/sh` (the default).

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
  `EntryKind`, `size` and the entry's bytes as an `Input`. `size` is the entry's
  uncompressed size as its container header states it, or `None` when the header gives
  none; it is a claim, never trusted: the router checks it with `file_size` before reading,
  and counts the bytes it actually reads. The `Input` is a stream, or a seekable reader where the
  container can give one (a zip on a seekable outer input, or one buffered in memory). The
  router normalizes a path with `entry_path`, gives no index to a `Directory`, and gives a
  `Link` `NotChecked{Symlink(Unresolved)}` and a `Special` entry `NotChecked{SpecialFile}`;
  neither is followed or read.
- `ReadCtx` holds the base `Location` of the file being read (input, frames, path; line
  `None`, scope `File`, or the script's scope), the file's declared `encoding`, and one
  `WorkBudget`. Readers build each fact's location with `ctx.at(line, scope)`. Readers never
  see an encoding pattern: the router resolves the declaration with
  `WorkerSettings::resolve_encoding`, the first declaration whose pattern matches the file's
  path in its innermost root, and passes only the result (`None` when nothing matches). It
  matches as the collector does: byte-exact, with no case folding, except AD-15's Windows
  fallback for paths on disk, which encoding declarations follow too; never inside a
  container. The budget is private: a reader can only spend it, through `work`, `buffer`,
  `release`, `file_size`, `entry` and `decompression`, and read it through
  `budget()`. Only the router builds a context with a
  given budget (`with_budget`) and takes it back (`into_budget`).
- `WorkBudget` counts against `Limits`: `work` steps (`Work`; tree-sitter steps through its
  progress callback), `buffer` and `release` for bytes held in memory (`Memory`, the counted
  per-file buffer), and depth. Each file and entry gets a fresh budget for work and bytes; a
  container's entries get `nested()`, one level deeper, which fails past `ArchiveDepth`. A
  value over its limit is `Exceeded{limit, value, max}`, which becomes
  `NotChecked{LimitExceeded(limit), Measured{value, limit: max}}`.
- `file_size(size)` refuses a file or entry over `FileSize`, before any of it is read.
- The decompression ratio is decompressed bytes over compressed bytes, per compressed
  stream (a gzip, xz, bzip2 or zstd stream, a zip entry's deflate data). The reader takes
  one `Decompression` per stream from `decompression()`, reports input with `consumed` (or
  `set_consumed` for a running total) and each block of output with `produced(bytes)`. A
  `Decompression` holds a handle to its job's counters, so it charges without the budget
  or the context in reach. `produced` first charges the bytes to the job's decompressed
  total, refused past `TotalDecompressedBytes`; then, once at least
  `RATIO_FLOOR` (1 MiB) of that stream was produced, it checks
  `produced > consumed × DecompressionRatio` and fails with the ratio rounded down, so small
  streams never trip it and a bomb stops while it is being produced.
- An entry whose bytes are decompressed lazily, as the router reads `Entry.data` after
  `next_entry` returns (a tar member inside a gzip stream, a zip entry's deflate data), is
  charged by core's adapters: the container wraps its compressed input in a `CountedRead`,
  and hands the decoder's output out as a `ChargedRead` built from that count and a
  `Decompression`. Each read charges `produced`; a refusal comes back as an `io::Error`
  from which `Exceeded::from_io` recovers the `Exceeded`, which the router records as the
  entry's `LimitExceeded` outcome.
- `entry()` counts each entry of a job against `TotalEntries`, and `produced` counts the
  job's decompressed bytes against `TotalDecompressedBytes`: each job is capped at the whole
  run-wide limit inside its worker (AD-4); the run-wide cut itself stays the collector's.
  Both counters are one per job, shared by every budget made from the job's budget:
  `nested()` (one level deeper) and `sibling()` (the next file or entry at the same depth,
  with fresh work and bytes). The router builds each entry's budget with one of them, never
  with `WorkBudget::new`, which starts a new job.
- Who charges what, so nothing is charged twice:

  | Charge | Charged by |
  | --- | --- |
  | `entry()` | the router, once per entry a container yields, before routing it |
  | `file_size` | the router, on a leaf file and on each entry before it reads it; never on a container's own stream, which is bounded by its entries |
  | `buffer`, `release` | the router for every buffer it fills and hands a reader (a whole leaf file, a buffered nested zip); a reader for its own allocations only |
  | `work` | the reader, for its own steps |
  | `decompression`, `produced` | the reader that decompresses, for streams it reads itself; for an entry's lazily read data, the `ChargedRead` the container hands out, read by the router |

- `FactSink` takes facts, regions and notes. The router's sink charges the entry's
  `EntryBudget`; when it refuses, the reader stops and returns the `Exceeded` it got. A
  reader that stops for any reason returns `NotRead`, and the router discards what the
  entry sent.

## Worker protocol (AD-3, AD-14, AD-17, RA2-C3, RA2-H4, RA2-M1, RA2-M8)

### Messages and frames

- Collector to worker: `Settings(WorkerSettings)` first, then `Job`s, and, before the first
  source job of an input tree, that tree's `Macros{tree, definitions, end}` frames.
  `WorkerSettings` holds limits and declared encodings only: never corpus data, and never
  the macro table (§ Macro definitions).
- Worker to collector: `Opened`, `Facts`, `Regions`, `Notes`, `EntryDone`, `CompileUnits`,
  `Definitions`, `JobDone`.
- Every message is one frame: a `u32` little-endian payload length, then the payload,
  written by `encode_frame` and read by `decode_frame`. The payload is `serde_json` until
  Story 1.4 chooses an encoding; any serde format works (Wire form).
- The frame cap is the `EntryMessageBytes` limit: both ends pass it as `max_len`, in both
  directions. The collector passes that same value to `MacroDef::validate` and to
  `encode_macro_table`, so a table built from validated definitions always fits.
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

- A `Job` names its input and its file. The collector, which walked the input, says which
  case it is, so the worker never guesses: `JobFile::WholeInput(name)` when the input is
  itself a file (the worker opens `input.given`; `name` is its file name, the job's path),
  and `JobFile::InInput(path)` for a file in a tree (the worker opens `input.given` joined
  with `path`). An input root the user gave is followed once, when it is a link, by both
  the walk and the worker; nothing below it is ever followed (AD-3). A `CompileDatabase` job's
  file is `JobFile::Setting`, the path as the bin resolved it from the setting, which may
  lie outside every input. A `Definitions` job has the same `JobFile` as its file's `Route`
  job (§ Macro definitions). `Job::is_well_formed` checks the pairing, and that only a
  `Route` job resumes.
- Every walked file that `classify` and the extension table make C/C++ gets a `Definitions`
  job, `NotInCompileDatabase` files included, since the macros of a header apply to the
  files that include it. An `OutOfScope` file gets none.
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
- `EntryDone{entry, outcome, bytes}` commits the entry. `bytes` is the bytes the worker
  actually inflated for the entry's own content, not counting its children, whatever the
  outcome: an entry refused part-way reports what was inflated before the refusal, and an
  entry the worker decompressed only to skip it (a resume, or a directory or link inside a
  compressed stream) has those bytes counted on the next entry it reports. The collector
  sums them in canonical order for `TotalDecompressedBytes`.
- An entry's facts, regions and notes count only once its `EntryDone` arrives, whatever its
  parent's state. When that `EntryDone`'s outcome is not `Read`, the collector discards what
  was sent for the entry.
- A job whose file is not a container sends its facts on `[]` and ends with
  `JobDone{outcome: Some(..)}`, which commits them. A container job ends with
  `JobDone{outcome: None}`: its own outcome went on `EntryDone{[0]}`.
- A not-checked outcome travels only on `EntryDone` or `JobDone`, never as a fact, region or
  note. This is a recorded deviation from the Story 1.1 AC, which names `EntryDone` alone:
  entry `[]`, the file of a job that is not a container, has no `EntryDone`, and its outcome
  rides on `JobDone{Some(..)}`.
- A container stopped by a limit after some children (an entry count, or a job's share of a
  run-wide limit) sends `EntryDone{[.., n], Aggregate{…}}` at its next index `n`, never 0,
  and nothing more for that container. `ends_container` names the container such a message
  ends. That index is never opened, and the aggregate takes the location of the
  container's entry 0.
- Per entry, facts are capped by `EntryFacts` and message bytes by `EntryMessageBytes`.
  Both ends keep one `EntryBudget` per entry and charge it only through
  `EntryBudget::charge_frame(msg, payload_len)`, with every `Facts`, `Regions` and `Notes`
  frame of that entry, and every `Definitions` frame of a definitions job (one budget for
  the job): it adds `payload_len` (the frame's payload bytes, without the
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
- The fault's reason comes from `classify_exit(Platform, WorkerExit{raw, killed_by_parent})`,
  or is `Fault(BadFrame)` for a bad frame, fact, outcome or region. When the parent killed
  the worker itself (the watchdog, or a cancel), it sets `killed_by_parent`, and the class is
  `KilledByParent` whatever the raw exit (a `SIGKILL` or a `TerminateProcess` code): it is
  never a fault and never costs the job's retry. The collector maps it with `ParentKill`:
  `Watchdog` is `LimitExceeded(Watchdog)`, marked `host_dependent`, and goes to the
  watchdog retry below; `Cancel` records nothing. `FaultKind` holds only what every OS can tell:
  `Crash` (a signal on Unix, an NTSTATUS error code on Windows: segfault, abort and stack
  overflow all look alike), `Panic` (exit code 101, a panic that escaped the worker's own
  catch), `BadFrame` and `Exited` (any other exit before `JobDone`, 0 included).
- The worker's allocation-failure path exits with `WORKER_MEMORY_CAP_EXIT`, a code reserved
  for it. The worker installs that path both behind Rust's global allocator and, through
  `ts_set_allocator`, for tree-sitter's C allocations, so an allocation failure in either
  exits with that code rather than aborting as a `Crash`. That exit is `ExitClass::WorkerMemoryCap`: the outcome is
  `LimitExceeded(WorkerMemoryCap)`, marked `host_dependent`.
- A file ended by the OS memory cap or the watchdog is retried once in a fresh worker. Its
  outcome, from the retry, is marked `host_dependent` whatever the retry gives. When the
  retry is also ended, the outcome is `LimitExceeded(WorkerMemoryCap)` or
  `LimitExceeded(Watchdog)`. When the run-wide watchdog fires, the files not yet read become
  one `Aggregate{LimitExceeded(RunWatchdog)}`, marked `host_dependent`.

## Outcomes and counting (AD-10, RA2-H3)

- Every file and every container entry gets exactly one outcome. A container counts once,
  through its entry 0; `JobDone{None}` adds nothing.
- The collector checks every outcome and region a worker sends.
  `Outcome::validate_from_worker` refuses `OutOfScope` and `NotInCompileDatabase` (only
  the collector makes those), `host_dependent: true` (only the collector marks it), an
  `Aggregate` with `count: Some(n)` where `n` is 0 or below `at_least`, an `Aggregate` on a
  container's entry 0, a `Read` outcome with `file_kind: Source` and no line count
  (`NoLineCount`), an `Aggregate` with a line count (`LinesOnAggregate`), and a `Read`
  outcome whose count is incomplete (`PartialCountOnRead`).
  `Outcome::validate_lines(bytes)` refuses a line count above `bytes` (`LinesAboveBytes`),
  since every line has at least one byte: the collector passes `EntryDone.bytes` for an
  entry, and the `FileSize` limit for the outcome on `JobDone{Some(..)}`, which carries no
  byte count. `Region::validate` refuses `first_line` 0 and `last_line` before
  `first_line`; once the entry's outcome arrives,
  `CollectedFile::validate_regions` also refuses a region whose `last_line` is past the
  outcome's line count (`PastEnd`), and checks nothing against a count that is `None`. A
  failure is `Fault(BadFrame)` for the job, as for a fact.
- A reader fits its regions to the file's line count before sending them
  (`Region::fit_within`), since a parser may place an error at end of file on the line
  after the last one: it clamps both `first_line` and `last_line` to the last line, so a
  region past the end becomes a region on the last line and the file still counts as
  having an unreadable region. A 0-line file sends no region. So `PastEnd` only catches a
  worker that did not, and no clamp leaves `first_line` after `last_line`.
- `Outcome.lines` is a `LineCount{lines, complete}`, or `None` when no line was counted. A
  line count is the number of LF line ends, plus 1 when the counted bytes are non-empty and
  do not end in LF. For a UTF-16 file (declared `Utf16Le` or `Utf16Be`, or code page 1200
  or 1201, or undeclared and starting with a UTF-16 byte-order mark) it counts 16-bit code
  units after the mark, an odd final byte being one more unit that is not LF; every other
  file, a declared UTF-8 or code-page file starting with `FF FE` included, counts its bytes
  as given. So CR LF ends one line, a CR-only file is 1 line, `"a\nb"` is 2 and an empty
  file 0. `line_count(bytes, declared)` and the streaming `LineCounter` are the one
  implementation.
- The worker counts line ends while it streams any file it opens, up to `FileSize`, and
  reports the count whatever the outcome, `NotChecked` included. A count that reached the
  end of the file is complete (`with_lines`). A count stopped before the end, by
  `FileSize`, the ratio, `Work` or any other cut after the first byte, is kept and marked
  incomplete (`with_partial_lines`). A file never opened, cut before any byte, or ended by a
  fault before it reported, has `None`; an `Aggregate` always has `None`.
- `Outcome.file_kind` is the `FileKind` the router identified, from the entry's `Opened`,
  or `None` when no reader claimed the file. For a job that faulted before `Opened{[]}`,
  the collector sets it from the same routing decision it would make by name, and builds
  the location from the job (no frames, the job's path); a container that is not a package
  (tar, zip) has `None`.
- An outcome the collector makes itself (a run-wide cut or `RunWatchdog` aggregate, a
  `NotInCompileDatabase` aggregate, a walk-level symlink, special file or `OutOfScope`) takes
  the key and location of the first file or entry it covers in canonical order: its key is
  `CanonicalKey::at(input index, that job's path, that entry's EntryPath)` and its location
  is that file's or entry's. A run-wide cut that starts at `[2, 1]` of a container job has
  key `at(index, path, [2, 1])`, after the committed `[2, 0]`, and the location from
  `[2, 1]`'s `Opened`; a cut that starts at a job not yet dispatched uses `[]` and the
  location the walk saw.
- `CollectedFile{key, location, outcome, regions, notes}` carries each committed outcome
  from the collector, through the bin, to verdict: `key` is `CanonicalKey::for_outcome`,
  `location` comes from the entry's `Opened` (for an aggregate at an index never opened,
  from its container's entry 0), and `regions` and `notes` are what the entry sent. A run's
  files are a `SortedVec<CollectedFile>`, in canonical order. A compile-database or
  definitions job never yields one.
- Reasons are a closed enum with no free text; a reason that names a limit, a fault, a
  special file type or a link target carries that closed value. `Details` holds typed
  measurements only.
- `verdict_counts` gives the verdict-line counts: files read are the `Read` outcomes; files
  not checked are the `NotChecked` outcomes plus each `Aggregate`'s count. An `Aggregate`
  with no count adds `at_least`, never below 1, and makes the total `AtLeast`.
  `OutOfScope` adds nothing: it feeds only the FR-5 excluded-path counts.
  Every sum in `verdict_counts` and `sm3_share` saturates at `u64::MAX` rather than
  overflowing.
- SM-3 (AD-13, PRD §8 as amended) is `sm3_share(&CollectedFile…) -> Sm3`: a `LineShare`
  (`lines`), a `FileShare` (`files`), and the lists the report prints beside them. Both
  shares count only files whose `file_kind` is `Source` (C/C++ in release 1), and both leave
  out `OutOfScope` files and `NotInCompileDatabase` ones, as `NotChecked` or as `Aggregate`.
  - The line share's denominator is the lines of every such file. Its numerator is every
    line of a file not checked, plus, for a `Read` file, the union of its regions' line
    ranges, each clamped to the file's line count (overlapping regions count once).
  - A file with no line count adds 0 lines to both counts and its file count to
    `uncounted_files`: 1 for a single file, and for an `Aggregate` the same count the file
    share gives it. An incomplete count adds its lines to both counts and 1 to
    `incomplete_files`. A line share with either above 0 is a lower bound
    (`LineShare::is_lower_bound`), printed "at least". `Sm3.uncounted` and
    `Sm3.incomplete` hold the `CanonicalKey` of each such file (one per `Aggregate`, its
    first file), in canonical order, so the report lists them without computing anything.
  - The line share is `None` ("n/a") only when its denominator, `uncounted_files` and
    `incomplete_files` are all 0: with no source file at all, or with only empty ones. With
    uncounted files and no counted line it is present: "at least 0 of 0, N files
    uncounted".
  - The file share is the files not checked for any reason (an `Aggregate` counts its
    `count`, or `at_least` and never below 1) plus the read files with any unreadable
    region (`CollectedFile::has_unreadable_region`), over all such files, computed as
    before Story 1.11. A zero denominator gives `None` ("n/a").
  - Worked example: a 100-line file read with regions 3–5 and 4–9, a 40-line file
    `NotChecked{Unparseable}`, an empty file read, a file refused by `FileSize` before any
    byte was read, a 1,000-line file cut by `FileSize` after 400 lines, and an excluded
    file. Lines: (7 + 40 + 400) / (100 + 40 + 0 + 400) = 447 / 540, with
    `uncounted_files` 1 and `incomplete_files` 1, so "at least 447 of 540", listing both
    cut files. Files: 4 / 5; the excluded file is in neither.

## Macro definitions (AD-14, AD-21, D-15)

- A `MacroDef` is one `#define` as written: `name`; `location`, the `#define` line in its
  file (input, no frames, the file's path, the line, scope `File`); `params`, `None` for an
  object-like macro, else `MacroParams{names, variadic}` with the named parameters as
  written and `variadic` `None`, `Ellipsis` (`...`, used as `__VA_ARGS__`) or `Named(name)`
  (GNU `name...`); and `body`, the replacement text as written, line continuations joined
  and comments kept. No `#if` is evaluated or used to choose a definition: every `#define`
  in the file is reported, whatever directives enclose it, and tried. `#undef` is ignored.
- `MacroTable` is a `SortedVec<MacroDef>`, ordered by `MacroDef`'s derived `Ord`: name
  bytes, then `location`, then `params` and `body`. Within one tree, `location` orders as
  `CanonicalKey` does (input, path, line), so that is name, then canonical order.
  Duplicates are kept, since the reader tries every definition (D-15).
- The collector dispatches one `Definitions` job per walked C/C++ file (§ Jobs), with the
  same `JobFile` as that file's `Route` job. The router identifies the file as it does for
  a source job, and the one core decode function applies, so the outcome's `file_kind` and
  its FR-10 encoding reasons come from the same routing decision. A file the router does
  not identify as C/C++ answers no definitions. Otherwise the worker reads the file's
  `#define`s, never following an include, and answers with zero or more
  `WorkerMsg::Definitions`, in file order, then `JobDone{Some(outcome)}`. The only other
  message it may send is one `Notes{[], [DroppedDefinitions(n)]}` (below); never
  `Opened`, `Facts`, `Regions`, `EntryDone`, `JobDone{None}` or any other note, and a
  `Route` job never sends `DroppedDefinitions`. `WorkerMsg::allowed_in(kind)` says which
  messages each job kind may send; any other is `Fault(BadFrame)` for the job. The outcome
  is checked with `validate_from_worker` like a route job's, so a `Read` `Source` outcome
  carries its line count: the count is never used, but one validation serves every job
  kind.
- `MacroDef::validate(job, max_len)` is the one predicate for a definition, applied by the
  worker before it sends and by the collector before it merges. It refuses an empty name
  (`EmptyName`); a name that is not a C identifier (`NotIdentifier`): valid UTF-8 made of
  ASCII letters, digits, `_`, `$` (a GNU extension) and non-ASCII characters that are not
  whitespace or control characters, not starting with a digit; a parameter, or a named
  variadic, that is not one, or is named `__VA_ARGS__` (`BadParameter`); a parameter named
  twice, or a named variadic that repeats a parameter (`DuplicateParameter`); a missing
  line or line 0, no path, any frame, a scope other than `File`, a job that is not a
  `Definitions` job for the same input and path; a size that cannot fit a frame
  (`TooLong`); and a definition that cannot be encoded (`Encode`).
- The worker drops each definition that fails the predicate, oversized ones included,
  sends the rest, and answers `Read` with `Note::DroppedDefinitions(n)`, `n` the count
  dropped, so one bad `#define` costs only itself. The collector adds that note to the
  `CollectedFile` of the same file's `Route` job. A definition that still fails the
  predicate at the collector is `Fault(BadFrame)` for the job.
- `encode_definitions` splits the definitions into frames within the frame cap; the
  collector concatenates them in arrival order. Its size check counts each definition
  against the larger of the worker and the collector envelope, so anything a worker can
  send also fits a table frame, and since the worker filtered with the predicate first it
  never refuses. Every splitter clamps the cap to `u32::MAX`, the largest length the prefix
  can carry. Validated definitions are what lets `encode_macro_table`, which has no
  per-definition check of its own, never refuse.
- The collector keeps a definitions job's definitions only when its outcome is `Read`. Any
  other outcome, a fault included, loses only that file's definitions; the other files'
  still merge. A definitions job's outcome never joins the run's outcomes, never counts on
  a verdict line or in SM-3, and has no `CanonicalKey`.
- When a definitions job ends by the watchdog or the OS memory cap, every `Route` outcome
  of that tree, `Read` or `NotChecked`, is marked host-dependent through the existing
  `Outcome.host_dependent`, since the tree's table may differ on another host (AD-3). The
  job is retried once as in § Faults and resume, and a successful retry leaves the marking:
  whether the first try ended depended on the host.
- `MacroTableBytes` is one run-wide limit on the sum over all trees.
  `merge_macro_tables(definitions, MacroTableBytes)` merges every tree's definitions in
  canonical order (input order, then path, by `location`), keeping whole files: it stops
  before the first file whose definitions would pass the cap, so a file's definitions are
  all kept or all dropped and the kept files are one canonical prefix of the run. A
  definition is measured by `MacroDef::table_bytes`, the byte length of its name, its
  parameter names (the named variadic included) and its body, so the cut never depends on
  how an input path was typed or on the wire encoding. The result is `MergedTables{tables,
  cut}`: `table(tree)` is that tree's table (empty when it kept nothing), and `cut` lists
  every tree that lost definitions. Files are still read; the macros past the cap are not
  expanded, and `Result.macro_tables_cut` carries `cut`, so the report says so.
- Delivery: the collector sends a tree's table, after `Settings` and before any source job
  of that tree, as one or more `CollectorMsg::Macros{tree, definitions, end}` frames split
  by `encode_macro_table`; `tree` is the input's index, and `end` is true on the last
  frame only. The table is input structure, never corpus data, and never part of
  `WorkerSettings`. The rules, which a worker keeps through `MacroTables`:
  - every definitions job of a tree, and of every tree before it in input order (the
    run-wide cut is a canonical prefix), commits before the first source job of that tree
    is sent, so the table is final when it is sent; this barrier serialises the start of
    each tree;
  - a worker concatenates a tree's frames in order, and the table is complete at the frame
    with `end`;
  - a `Macros` frame for a tree that already has a table starts a new table, which
    replaces the old one: the old one is dropped at that first frame;
  - a `Route` job for a tree with no complete table reads without expansion;
  - a frame holding a definition from another input than `tree` is refused
    (`ForeignDefinition`): the worker stops, and the collector sees the job fail as a fault;
  - a replacement worker gets `Settings` and every complete table before its first job.
- The reader uses the table for parsing only; facts keep the invocation's location and the
  guards as written (D-15).

## Compile database (AD-21, RA2-H7, S-F10)

- `Settings.compile_database` is a `CompileDatabaseSetting{file, tree}`: the database's path
  as the bin resolved it, and the index of the input whose source tree it limits.
  `Settings::compile_database_job` builds its job: `Ok(None)` with no database, and
  `Err(TreeOutOfRange)` when `tree` names no input, which is a typed run error. The collector dispatches it first and
  alone, before walking that tree.
- The worker answers with one or more `CompileUnits`, holding each entry's `directory` and
  `file` as written and in the database's order, then `JobDone{Some(outcome)}`.
  `encode_compile_units` splits the list into frames, each within the frame cap; the
  collector concatenates them in arrival order. A single unit over the cap makes the job
  `NotChecked` with `LimitExceeded(EntryMessageBytes)`.
- That job's outcome never joins the run's outcomes and never counts on a verdict line.
  `CompileDatabaseError::check` turns it into the run's result: `Read` is fine, anything
  else is the typed run error `NotRead(reason)`: `NotChecked` and an aggregate give their
  own reason, and an `OutOfScope` outcome or a missing outcome, which a worker may not send
  for this job, give `Fault(BadFrame)`. A database that matches no walked file is
  `MatchesNoFile`. The bin maps all three to exit 2 (FR-4). The error prints each reason by
  its serialized snake_case name.
- The collector alone normalizes the units and matches them to walked paths.
  `SOURCE_EXTENSIONS` and `HEADER_EXTENSIONS` are the one extension table, matched ignoring
  ASCII case. The collector applies `classify` by name to walked paths, before anything is
  dispatched: it cannot know a file's content. `classify` takes the relative path or the
  file name. It takes the part after the last `/` only (a backslash is an ordinary
  byte, as in `RelPath`): a listed source extension is `Source`; anything else, unlisted or
  extensionless included, is `Header`.

## Limits, settings and versions (AD-4, AD-17, AD-22)

- `LIMITS` is the one limits table: names from FR-38, with its memory limit and watchdog
  each split in two (below), plus `EntryFacts`, `EntryMessageBytes` and `MacroTableBytes`
  (a `RunWide` limit on the merged macro tables, § Macro definitions), each with its flag
  name, scope, unit and default. The defaults are provisional until the readers measure
  them; Story 1.6 sets them, `MacroTableBytes` included.
- `Memory` is the per-file buffer the readers count through `WorkBudget`; it is
  deterministic. `WorkerMemoryCap` is the OS cap each worker sets on itself before its
  first job (Job Object or `setrlimit`), a last resort whose hit is host-dependent.
  `Watchdog` is the per-file wall-clock last resort and `RunWatchdog` the run-wide one.
- `WorkerMemoryCap` is at least `MEMORY_CAP_FACTOR` (4) times `Memory`, so a file within
  its counted buffer cannot reach the OS cap through that buffer alone. Cross-limit rules
  like this one are checked once, on the finished table, by `Limits::validate`, so the
  order in which the bin merges flag, project file and default never changes the result.
  `set` checks only the one value; the bin calls `validate` after merging, and
  deserialization calls it too. A failure is `LimitError::CapBelowMemory`. The same check
  refuses a `MacroTableBytes` above `WorkerMemoryCap` (`LimitError::TableAboveCap`), since
  every worker holds the tables; Story 1.6 sets the default.
- Every limit is at least 1. `Limits::set`, `Limits::with` and deserialization refuse 0
  with `LimitError::Zero`, and deserialization refuses unknown fields. `Limits` is read
  through `get`; its fields are private. `EntryMessageBytes` is at most `u32::MAX`, the
  largest length the frame prefix can carry; above it is `LimitError::AboveMax`.
- `Settings.jobs` is `Option<NonZeroU32>`: `None` means one worker per core.
- `Settings` is built only by the bin. Path patterns keep their base and are never
  rewritten. A worker receives `Settings::worker()` and nothing else.
- The rule-schema, profile-format, waiver-file, project-file and JSON-output versions are
  the `SchemaVersion{major, minor}` constants in `version`, printed, parsed and serialized
  as the string `"major.minor"` (`"1.0"`). An added field or kind bumps the minor; anything else bumps the major.
  A loader at version `v` accepts a file at version `f` when `v.accepts(f)`: the same major,
  at any minor up to the loader's own (operator ruling). `v.check(f)` gives the typed
  `UnsupportedVersion{supported, found, reason}` otherwise, naming both versions, with
  reason `OtherMajor` or `NewerMinor`. Every loader checks against them and every report
  states them through `Versions::CURRENT`. Story 6.8 owns the published policy.
- `JSON_OUTPUT_VERSION` stays 1.0. Story 1.11 changed `Result.sm3` into `Sm3`, with a line
  and a file share and the uncounted and incomplete lists, and added
  `Result.macro_tables_cut`. That change is not additive, so the rule above would make it a
  major bump; by operator decision 1.0 is re-recorded instead, because no output has
  shipped (as `RULE_SCHEMA_VERSION` 1.0 was). `RULE_SCHEMA_VERSION` stays 1.0, since no
  `Fact` changed, and its snapshot is unchanged. The worker messages and `Job` carry no
  version: both ends are the same binary.

## Verdict types (AD-5, AD-16, AD-19, RA2-M6)

- `Tier`'s order runs worst to best, so the worst of several tiers is their minimum.
- Ids are kebab-case segments joined by `/`. Rule ids have at least two segments
  (`<dimension>/<name>`); user ids start with `user/`.
- `Finding`, `Evidence`, `Observation`, `Coverage`, `Profile`, `Control` and `Result` live
  here. Their fields are minimal; later stories extend them.
- `RuleCoverage.file_kinds` is a sorted list of alternatives, each a sorted list of
  `FileKind`s that must all be read for the rule to fire. `RuleCoverage::new` and
  deserialization refuse an empty alternative, a repeated alternative and a repeated
  `FileKind` within one, so each is in effect a set. `can_fire(read)` is true when
  every kind of some alternative was read. A rule matching `all` of a source fact and a
  package fact has the one alternative `[source, package]`; one matching `any` of an
  import or a call has `[[source], [binary]]`.
- `boxt-verdict` depends on `boxt-core` only, never on `boxt-rules`. It parses the waiver
  file itself, with its own size and depth limits.
