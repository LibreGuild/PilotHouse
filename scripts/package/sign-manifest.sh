#!/usr/bin/env bash
# Signs a release's latest.json for the app's updater: writes FILE.sig,
# the Ed25519 signature of FILE's exact bytes, in base64. The private key
# (PEM) comes in $UPDATE_SIGNING_KEY; unless its public key is the one the
# app carries (brand/update_key.pub, or the file brand/brand.toml names),
# nothing is signed, since no copy of the app could check the signature.
#
# usage: sign-manifest.sh FILE
set -euo pipefail

file=${1:?usage: sign-manifest.sh FILE}
: "${UPDATE_SIGNING_KEY:?UPDATE_SIGNING_KEY is not set}"
root=$(cd "$(dirname "$0")/../.." && pwd)
expected=$(tr -d '[:space:]' < "$root/brand/$("$root/scripts/package/brand.sh" updates key)")

umask 077
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf '%s\n' "$UPDATE_SIGNING_KEY" > "$work/key.pem"
openssl pkey -in "$work/key.pem" -pubout -out "$work/public.pem"

public=$(openssl pkey -pubin -in "$work/public.pem" -outform DER | tail -c 32 | base64 | tr -d '\n')
if [ "$public" != "$expected" ]; then
  echo "the signing key's public key ($public) is not the one the app carries ($expected)" >&2
  exit 1
fi

openssl pkeyutl -sign -inkey "$work/key.pem" -rawin -in "$file" -out "$work/signature"
# Checked the way the app checks it, before anything is published.
openssl pkeyutl -verify -pubin -inkey "$work/public.pem" -rawin -in "$file" -sigfile "$work/signature" >/dev/null
base64 < "$work/signature" | tr -d '\n' > "$file.sig"
echo "signed $file"
