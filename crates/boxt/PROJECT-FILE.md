# Project file

A project file holds the scan settings for one tree, so every run of that tree scans it the
same way. Its version is `PROJECT_FILE_VERSION` in `boxt-core/src/version.rs`, now `1.0`.

## Where boxt finds it

- `--project FILE` names the file. It always wins, and no search runs.
- Otherwise boxt searches upward from the first input (its folder, when the input is a
  file) to the filesystem root, whatever the working folder, and uses the first folder that
  holds `boxt.yaml` or `.boxt.yaml`. A folder that holds both is an error naming both.
- With none found, every setting has its default.
- The input's version-control root is the nearest folder at or above the first input that
  holds `.git`, `.hg` or `.svn`. A file found in a folder above that root is still used,
  with a warning on stderr and a note in the report's header box.

The report names the file used, `none found`, or the path given with `(named by --project)`.
A found path is the folder where the file was found, the first input's folder or any folder
above it, joined with the file's name. The search starts from the first input made
absolute, with `..` and `.` resolved by name, not by following links. On Windows every
shown path uses `/` as its separator, like finding locations. The hidden `__collect`, `__bench` and
`__measure` commands and the worker never read a project file.

## Format

YAML, at most 1 MiB, one document, read with the corpus loader's strict options: duplicate
keys, unsupported tags and nesting deeper than 64 levels are refused, and only `true` and
`false` are booleans (`yes` and `on` are strings).

```yaml
schema: "1.0"
targets: [kubernetes-restricted, rootless-podman]
jobs: 4
limits:
  file-size: 33554432
  work: 4000000
exclude: [third_party/**, "tests/*.c"]
encodings:
  - {path: legacy/**, encoding: windows-1252}
  - {path: "**/*.txt", encoding: latin-1}
```

| Key | Value | Flag | Default |
|---|---|---|---|
| `schema` | the file's version, quoted: `"1.0"`. Required | none | none |
| `targets` | a list of one or more target ids; an empty list is refused | `--target ID`, repeatable | every target |
| `jobs` | a whole number from 1 to 4294967295 | `--jobs N` | one per core |
| `limits` | a mapping from limit name to a whole number | `--limit NAME=VALUE`, repeatable | each limit's default |
| `exclude` | a list of path patterns; the files they match are not scanned | `--exclude PATTERN`, repeatable | none |
| `encodings` | a list of `{path: PATTERN, encoding: NAME}` mappings | `--encoding PATTERN=NAME`, repeatable | none |

`limits` takes the 14 names `--limit` takes: `file-size`, `work`, `memory`,
`archive-depth`, `decompression-ratio`, `total-entries`, `total-decompressed-bytes`,
`watchdog`, `run-watchdog`, `worker-memory-cap`, `entry-facts`, `entry-message-bytes`,
`macro-table-bytes` and `syntax-depth`. Their units and defaults are in
`boxt-core/LIMITS.md`.

Inputs come only from the command line: the file has no `inputs` key.

## Path patterns

`exclude` and `encodings` take path patterns, such as `third_party/**` or `src/*.c`.

- A pattern is relative to its base: the project file's folder for a pattern in the file,
  or the working folder for a flag. A pattern under `sub/boxt.yaml` that says `gen/**`
  matches `sub/gen/x.c`, not `gen/x.c`, whatever the input. The pattern is never
  rewritten.
- `/` is the separator on every host, and `\` escapes the next character. `*` and `?`
  stop at `/`, so `src/*.c` matches `src/a.c` but not `src/a/b.c`; `**` crosses folders.
  `{a,b}` and `[abc]` work as in a shell.
- A pattern is matched against each file's absolute path with `.` and `..` resolved by
  name. On Windows, a pattern that does not match exactly is tried again ignoring case;
  on other hosts case always counts.
- A bare folder name, such as `gen`, matches only the folder itself, not the files in it:
  exclude a folder with `gen/**`.
- An empty pattern, an absolute one (starting with `/` or a drive such as `C:`), one that
  starts with `./`, one with a `.` or `..` segment and one with an empty segment (a
  trailing `/` or `//`) are refused.

### Excludes

A file an exclude matches gets no job and produces no findings. It counts as neither read
nor not checked, and it does not count toward the `total-entries` limit. The report lists
each exclude with the number of files it left out, `0 files` included; the first matching
exclude takes the file. Excludes set the scan's scope; they are not waivers.

The project file in use is never read as an input. When it lies inside an input the report
lists it among the excluded paths, `excluded boxt.yaml · boxt's project file`, whatever
the excludes say. A folder whose contents an exclude would match, such as a linked or
unreadable `third_party` under `third_party/**`, is excluded with them rather than
counted as not checked.

### Encodings

The first declaration whose pattern matches a file sets its encoding. Names are matched
ignoring case: `utf-8`, `utf-16le`, `utf-16be`, `latin-1`, and `windows-NNNN` for each
code page boxt decodes, such as `windows-1252` or `windows-932`. The code pages are 866,
874, 932, 936, 949, 950, 1200 (UTF-16LE), 1201 (UTF-16BE), 1250 to 1258, 10000, 10007,
20127 (ASCII), 20866, 20932, 21866, 28591 (Latin-1), 28592 to 28598, 28603, 28605,
38598, 50220, 51932, 54936 and 65001 (UTF-8). A bare `utf-16` is
refused, since it names no byte order, and there is no `cpNNNN` form. A declared file
whose bytes do not decode is not checked, `invalid for its declared encoding`. An
undeclared file is read as UTF-8, or as UTF-16 when it starts with a byte-order mark;
otherwise it is not checked, `encoding undeclared`.

`--encoding PATTERN=NAME` splits at the last `=`, so a pattern may hold one.

## Precedence

For each setting, the flag wins, then the project file, then the default.

- `--target` given once or more replaces the file's `targets` list; it does not add to it.
  `--exclude` and `--encoding` replace the file's `exclude` and `encodings` lists the
  same way.
- Each `--limit NAME=VALUE` replaces that one limit; the file's other limits stay.
- The limits are checked together after the merge, so a limit set by flag may conflict with
  one set by the file.

## Errors

Each of these exits 2 with `boxt: …` on stderr and nothing on stdout, before any job is
dispatched:

- an input path that cannot be made absolute to start the search;
- a folder that cannot be checked for a project file during the search;
- a flagged or found file that cannot be read, or two project files in one folder;
- malformed YAML, a file over 1 MiB or not UTF-8;
- a missing `schema`, or a version this build does not read (the message names both);
- an unknown key, such as `colour` or `limits.wrk`;
- a bad value, naming its key, such as `jobs`, `limits.work` or an empty `targets`;
- a refused pattern, naming its key or flag, such as `exclude[1]`, `encodings[0].path` or
  `--exclude`;
- an encoding boxt cannot decode, naming its key or flag, such as `encodings[0].encoding`
  or `--encoding`;
- an unknown target, listing the known targets (prefixed with the file and `targets:` when
  the file named it);
- limits that conflict after the merge.

A newer minor version (`1.9` read by a `1.0` build) is refused; an older minor of the same
major is read.
