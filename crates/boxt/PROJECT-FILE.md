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
```

| Key | Value | Flag | Default |
|---|---|---|---|
| `schema` | the file's version, quoted: `"1.0"`. Required | none | none |
| `targets` | a list of one or more target ids; an empty list is refused | `--target ID`, repeatable | every target |
| `jobs` | a whole number from 1 to 4294967295 | `--jobs N` | one per core |
| `limits` | a mapping from limit name to a whole number | `--limit NAME=VALUE`, repeatable | each limit's default |

`limits` takes the 14 names `--limit` takes: `file-size`, `work`, `memory`,
`archive-depth`, `decompression-ratio`, `total-entries`, `total-decompressed-bytes`,
`watchdog`, `run-watchdog`, `worker-memory-cap`, `entry-facts`, `entry-message-bytes`,
`macro-table-bytes` and `syntax-depth`. Their units and defaults are in
`boxt-core/LIMITS.md`.

Inputs come only from the command line: the file has no `inputs` key.

## Precedence

For each setting, the flag wins, then the project file, then the default.

- `--target` given once or more replaces the file's `targets` list; it does not add to it.
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
- an unknown target, listing the known targets (prefixed with the file and `targets:` when
  the file named it);
- limits that conflict after the merge.

A newer minor version (`1.9` read by a `1.0` build) is refused; an older minor of the same
major is read.
