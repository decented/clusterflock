#!/usr/bin/env bash
# Publish a Cluster Flock release with nostr-release-kit's publisher and this
# repo's tools/release/release.json (channel, package, relays, certificate pin,
# release-key location). Usage:
#   tools/release/publish.sh <apk> --version X.Y.Z [--notes "..."] [--dry-run] [--allow-cert-change] [--allow-older]
#   tools/release/publish.sh --print-pubkey
# The kit's checkout: $KIT, default ../forgesworn/nostr-release-kit beside this repo
# (install its publisher's dependencies once: `npm ci` in its tools/). Env names are
# the kit's: RELEASE_KIT_RELAYS, RELEASE_KIT_APKSIGNER, RELEASE_KIT_AAPT2; the key
# file comes from $FLOCK_RELEASE_KEY_FILE (default ~/.flock-release/release-key.hex).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KIT="${KIT:-$HERE/../../../forgesworn/nostr-release-kit}"
[ -f "$KIT/tools/publish-release.mjs" ] || { echo "ERROR: nostr-release-kit not found at $KIT (set KIT)" >&2; exit 1; }
exec node "$KIT/tools/publish-release.mjs" --config "$HERE/release.json" "$@"
