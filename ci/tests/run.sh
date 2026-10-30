#!/bin/sh
set -u

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
ci=$(dirname -- "$here")
root=$(dirname -- "$ci")
fixtures=$here/fixtures

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT TERM

failures=0

expect() {
  name=$1
  want=$2
  needle=$3
  shift 3
  out=$("$@" 2>&1)
  rc=$?
  verdict=ok
  case $want in
    pass)
      [ "$rc" -eq 0 ] || verdict="expected pass, exited $rc"
      ;;
    fail)
      if [ "$rc" -eq 0 ]; then
        verdict="expected failure, passed"
      elif ! printf '%s\n' "$out" | grep -qF -- "$needle"; then
        verdict="failed without naming '$needle'"
      fi
      ;;
    error)
      [ "$rc" -eq 2 ] || verdict="expected exit 2, exited $rc"
      ;;
    *)
      verdict="unknown expectation '$want'"
      ;;
  esac
  if [ "$verdict" = ok ]; then
    echo "ok    $name"
  else
    echo "FAIL  $name: $verdict"
    printf '%s\n' "$out" | sed 's/^/      /'
    failures=$((failures + 1))
  fi
}

crate() {
  mkdir -p "$work/$1/src"
  cp "$fixtures/$1/manifest.toml" "$work/$1/Cargo.toml"
  cp "$fixtures/$1/Cargo.lock" "$work/$1/Cargo.lock"
  : > "$work/$1/src/lib.rs"
  (cd "$root" && cargo metadata --format-version 1 --locked --manifest-path "$work/$1/Cargo.toml") > "$work/$1.json"
}

deny() {
  (cd "$root" && cargo deny --locked --manifest-path "$work/$1/Cargo.toml" --config "$root/deny.toml" check bans sources)
}

cd "$root" || exit 2

expect "AD-1 workspace" pass "" \
  "$ci/check-edges.sh"
expect "AD-1 reader to reader" fail "forbidden edge boxt-read-elf -> boxt-read-pkg" \
  "$ci/check-edges.sh" "$fixtures/edges-reader-to-reader.json"
expect "AD-1 core gains an edge" fail "boxt-core depends on workspace crate boxt-verdict" \
  "$ci/check-edges.sh" "$fixtures/edges-core-gains-edge.json"
expect "AD-1 build edge onto the bin" fail "boxt-render depends on the boxt bin crate" \
  "$ci/check-edges.sh" "$fixtures/edges-depends-on-bin.json"
expect "AD-1 dev edge" fail "forbidden edge boxt-rules -> boxt-verdict" \
  "$ci/check-edges.sh" "$fixtures/edges-dev-edge.json"
expect "AD-1 cfg(windows) edge" fail "forbidden edge boxt-verdict -> boxt-rules" \
  "$ci/check-edges.sh" "$fixtures/edges-target-specific.json"
expect "AD-1 path dependency outside the members" fail "boxt-render -> vendored-net" \
  "$ci/check-edges.sh" "$fixtures/edges-path-outside-workspace.json"
expect "AD-1 stale allow-list entry" fail "stale allow-list entry boxt -> boxt-render" \
  "$ci/check-edges.sh" "$fixtures/edges-stale-allow-list.json"
expect "AD-1 empty workspace members" error "" \
  "$ci/check-edges.sh" "$fixtures/edges-no-members.json"
expect "AD-1 missing workspace members" error "" \
  "$ci/check-edges.sh" "$fixtures/edges-null-members.json"
expect "AD-1 not metadata" error "" \
  "$ci/check-edges.sh" "$fixtures/edges-not-metadata.txt"

expect "AD-12 native workspace" pass "" \
  "$ci/check-native.sh"
expect "AD-12 links key" fail "fake-z" \
  "$ci/check-native.sh" "$fixtures/native-links.json"
for tool in bindgen cc cmake cxx-build nasm-rs pkg-config vcpkg; do
  expect "AD-12 $tool build-dependency" fail "build-depends on $tool" \
    "$ci/check-native.sh" "$fixtures/native-build-$tool.json"
done
expect "AD-12 tree-sitter allowed" pass "" \
  "$ci/check-native.sh" "$fixtures/native-tree-sitter.json"
expect "AD-12 other tree-sitter grammar" fail "tree-sitter-rust" \
  "$ci/check-native.sh" "$fixtures/native-tree-sitter-other.json"
expect "AD-12 optional inactive cc allowed" pass "" \
  "$ci/check-native.sh" "$fixtures/native-optional-cc.json"
expect "AD-12 unresolved links key ignored" pass "" \
  "$ci/check-native.sh" "$fixtures/native-links-unresolved.json"
expect "AD-12 native without resolve" error "" \
  "$ci/check-native.sh" "$fixtures/native-no-resolve.json"

expect "AD-12 std::net workspace" pass "" \
  "$ci/check-net.sh"
expect "AD-12 std::net use" fail "net-use/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-use"
expect "AD-12 std::net braced use" fail "net-brace/src/lib.rs:3:" \
  "$ci/check-net.sh" "$fixtures/net-brace"
expect "AD-12 std::net in tests" fail "net-tests/tests/net.rs:3:" \
  "$ci/check-net.sh" "$fixtures/net-tests"
expect "AD-12 std::net multi-line braced use" fail "net-multiline/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-multiline"
expect "AD-12 std::net nested braces" fail "net-nested/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-nested"
expect "AD-12 std::net std::os::unix::net" fail "net-unix/src/lib.rs:3:" \
  "$ci/check-net.sh" "$fixtures/net-unix"
expect "AD-12 std::net std :: net with spaces" fail "net-spaced/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-spaced"
expect "AD-12 std::net use std as" fail "net-alias/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-alias"
expect "AD-12 std::net extern crate std as" fail "net-extern-alias/src/lib.rs:1:" \
  "$ci/check-net.sh" "$fixtures/net-extern-alias"

expect "AD-12 cargo-deny workspace" pass "" cargo deny --locked --config "$root/deny.toml" check bans sources

if crate deny-openssl-sys; then
  expect "AD-12 cargo-deny openssl-sys" fail "openssl-sys" deny deny-openssl-sys
else
  echo "FAIL  could not resolve fixture deny-openssl-sys"
  failures=$((failures + 1))
fi

if crate deny-c-features; then
  expect "AD-12 cargo-deny rpm C feature" fail "xz-compression" \
    deny deny-c-features
  expect "AD-12 cargo-deny rpm C crate" fail "liblzma-sys" \
    deny deny-c-features
  expect "AD-12 cargo-deny zip C feature" fail "bzip2-rs" \
    deny deny-c-features
  expect "AD-12 native real bzip2-sys" fail "bzip2-sys" \
    "$ci/check-native.sh" "$work/deny-c-features.json"
else
  echo "FAIL  could not resolve fixture deny-c-features"
  failures=$((failures + 1))
fi

if crate deny-stack; then
  expect "AD-12 cargo-deny Stack backends allowed" pass "" deny deny-stack
  expect "AD-12 native real tree-sitter allowed" pass "" \
    "$ci/check-native.sh" "$work/deny-stack.json"
else
  echo "FAIL  could not resolve fixture deny-stack"
  failures=$((failures + 1))
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures self-test(s) failed"
  exit 1
fi

echo "all self-tests passed"
