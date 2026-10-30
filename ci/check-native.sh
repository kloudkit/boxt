#!/bin/sh
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
allowed=$here/allowed-native.txt

if [ $# -gt 0 ]; then
  metadata=$(cat -- "$1")
else
  metadata=$(cargo metadata --format-version 1 --locked \
    --filter-platform x86_64-unknown-linux-musl \
    --filter-platform aarch64-unknown-linux-musl \
    --filter-platform x86_64-pc-windows-msvc)
fi

if ! printf '%s\n' "$metadata" | jq -e '
  (.packages | type == "array") and (.resolve.nodes | type == "array")
' > /dev/null 2>&1; then
  echo "AD-12: not cargo metadata JSON with a resolve graph" >&2
  exit 2
fi

offenders=$(printf '%s\n' "$metadata" | jq -r '
  (reduce .packages[] as $p ({}; .[$p.id] = $p)) as $pkgs
  | ["bindgen", "cc", "cmake", "cxx-build", "nasm-rs", "pkg-config", "vcpkg"] as $tools
  | .resolve.nodes[]
  | $pkgs[.id] as $p
  | (if $p.links != null then "\($p.name) \($p.version) links \"\($p.links)\"" else empty end),
    (.deps[]
      | select(any(.dep_kinds[]; .kind == "build"))
      | $pkgs[.pkg].name as $dep
      | select($tools | index($dep))
      | "\($p.name) \($p.version) build-depends on \($dep)")
')
offenders=$(printf '%s\n' "$offenders" | sort -u)

status=0
count=0

while IFS= read -r line; do
  [ -n "$line" ] || continue
  name=${line%% *}
  if grep -qxF -- "$name" "$allowed"; then
    count=$((count + 1))
  else
    echo "AD-12: native code outside the allow-list: $line" >&2
    status=1
  fi
done <<END
$offenders
END

if [ "$status" -eq 0 ]; then
  echo "AD-12: no links key or native build-dependency outside the allow-list ($count allowed)"
fi

exit "$status"
