#!/bin/sh
# Prints one value from brand/brand.toml, the name, ids and links of this
# build (see docs/customizing.md), so the packaging scripts and the app read
# the same file:
#
#   scripts/package/brand.sh app name      # PilotHouse
#   scripts/package/brand.sh app vendor    # LibreGuild
#   scripts/package/brand.sh stem          # the name without spaces, for file names
#
# Only quoted values of a [section] are read (not [[arrays]]); the app's
# build checks the file's form, and this fails when the key is missing.
set -eu
file="$(dirname "$0")/../../brand/brand.toml"
value() {
  awk -v section="[$1]" -v key="$2" '
    /^[ \t]*\[/ { current = $0; sub(/[ \t]*(#.*)?$/, "", current); sub(/^[ \t]*/, "", current); next }
    current == section && match($0, "^[ \t]*" key "[ \t]*=[ \t]*\"") {
      rest = substr($0, RLENGTH + 1)
      sub(/".*$/, "", rest)
      print rest
      found = 1
      exit
    }
    END { if (!found) exit 1 }
  ' "$file" || { echo "brand/brand.toml has no $2 in [$1]" >&2; exit 1; }
}
if [ "${1:-}" = stem ]; then
  value app name | tr -d ' '
else
  value "${1:?usage: brand.sh SECTION KEY | brand.sh stem}" "${2:?usage: brand.sh SECTION KEY}"
fi
