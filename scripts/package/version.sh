#!/bin/sh
# Prints the workspace version from Cargo.toml (the one source of truth).
sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$0")/../../Cargo.toml" | head -1
