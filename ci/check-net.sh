#!/bin/sh
set -eu

if [ $# -eq 0 ]; then
  cd -- "$(dirname -- "$0")/.."
  set -- crates
fi

for dir in "$@"; do
  if [ ! -d "$dir" ]; then
    echo "AD-12: no such directory $dir" >&2
    exit 2
  fi
done

files=$(find "$@" -type f -name '*.rs' | sort)

status=0

while IFS= read -r file; do
  [ -n "$file" ] || continue
  hits=$(jq -Rrs --arg file "$file" '
    . as $src
    | [
        (match("\\bstd\\s*::\\s*net\\b"; "g")),
        (match("\\bstd\\s*::\\s*os\\s*::\\s*\\w+\\s*::\\s*net\\b"; "g")),
        (match("\\bstd\\s*::\\s*(?<group>\\{(?:[^{}]|\\g<group>)*\\})"; "g")
          | select(.captures[0].string | test("\\bnet\\b"))),
        (match("\\buse\\s+(?:::)?std\\s+as\\b"; "g")),
        (match("\\bextern\\s+crate\\s+std\\s+as\\b"; "g"))
      ]
    | unique_by(.offset)
    | .[]
    | ($src[0:.offset] | [scan("\n")] | length + 1) as $line
    | "\($file):\($line): \(.string | gsub("\\s+"; " "))"
  ' "$file") || {
    echo "AD-12: could not scan $file" >&2
    exit 2
  }
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/AD-12: std::net in /' >&2
    status=1
  fi
done <<END
$files
END

if [ "$status" -eq 0 ]; then
  echo "AD-12: no std::net in workspace sources"
fi

exit "$status"
