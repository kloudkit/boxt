#!/bin/sh
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
allowed=$here/allowed-edges.txt

if [ $# -gt 0 ]; then
  metadata=$(cat -- "$1")
else
  metadata=$(cargo metadata --format-version 1 --no-deps --locked)
fi

if ! printf '%s\n' "$metadata" | jq -e '
  (.packages | type == "array" and length > 0)
  and (.workspace_members | type == "array" and length > 0)
' > /dev/null 2>&1; then
  echo "AD-1: not cargo metadata JSON with workspace members" >&2
  exit 2
fi

members='
  .workspace_members as $ids
  | [.packages[] | select(.id as $id | $ids | index($id))]
'

edges=$(printf '%s\n' "$metadata" | jq -r "$members"' as $members
  | ($members | map(.name)) as $names
  | $members[] as $p
  | $p.dependencies[]
  | select(.name as $n | $names | index($n))
  | "\($p.name) -> \(.name)"
')
edges=$(printf '%s\n' "$edges" | sort -u)

outside=$(printf '%s\n' "$metadata" | jq -r "$members"' as $members
  | ($members | map(.name)) as $names
  | $members[] as $p
  | $p.dependencies[]
  | select(.path != null and (.name as $n | $names | index($n) | not))
  | "\($p.name) -> \(.name) (\(.path))"
')

status=0

while IFS= read -r edge; do
  [ -n "$edge" ] || continue
  from=${edge%% -> *}
  to=${edge#* -> }
  if ! grep -qxF -- "$edge" "$allowed"; then
    echo "AD-1: forbidden edge $edge" >&2
    status=1
  fi
  if [ "$from" = boxt-core ]; then
    echo "AD-1: boxt-core depends on workspace crate $to" >&2
    status=1
  fi
  if [ "$to" = boxt ]; then
    echo "AD-1: $from depends on the boxt bin crate" >&2
    status=1
  fi
done <<END
$edges
END

while IFS= read -r line; do
  [ -n "$line" ] || continue
  echo "AD-1: path dependency outside the workspace members: $line" >&2
  status=1
done <<END
$outside
END

while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  if ! printf '%s\n' "$edges" | grep -qxF -- "$entry"; then
    echo "AD-1: stale allow-list entry $entry" >&2
    status=1
  fi
done < "$allowed"

if [ "$status" -eq 0 ]; then
  echo "AD-1: $(printf '%s\n' "$edges" | grep -c .) workspace edges, exactly the allow-list"
fi

exit "$status"
