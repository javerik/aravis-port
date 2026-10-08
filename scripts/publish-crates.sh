#!/usr/bin/env bash
# Publishes every workspace crate to crates.io in dependency order.
#
# A crate whose current version is already on crates.io is skipped, so a release that failed
# halfway can simply be run again. `cargo publish` waits until each crate is in the index before
# the next one, which needs it, is published.
#
#   CARGO_REGISTRY_TOKEN=... scripts/publish-crates.sh
#   DRY_RUN=1 scripts/publish-crates.sh     # package and verify only, no upload
set -euo pipefail

cd "$(dirname "$0")/.."

# Real (non-dev) dependency order; see the [dependencies] of each crate.
CRATES=(
    aravis-port-core
    aravis-port-genicam
    aravis-port-stream
    aravis-port-fakecamera
    aravis-port-device
    aravis-port
)

version=$(cargo metadata --no-deps --format-version 1 |
    jq -r '.packages[] | select(.name == "aravis-port") | .version')

# True if crates.io's sparse index lists `$1` at `$version` (names of 4+ chars live under
# <first two>/<next two>/<name>).
published() {
    local url="https://index.crates.io/${1:0:2}/${1:2:2}/$1"
    curl -fsSL "$url" 2>/dev/null | jq -e --arg v "$version" 'select(.vers == $v)' >/dev/null
}

for crate in "${CRATES[@]}"; do
    if published "$crate"; then
        echo "$crate $version is already on crates.io, skipping"
    elif [[ -n "${DRY_RUN:-}" ]]; then
        echo "$crate $version would be published"
    else
        echo "== publishing $crate $version"
        cargo publish -p "$crate"
    fi
done

# A per-crate dry run can't resolve internal dependencies that aren't on crates.io yet; the
# workspace dry run packages them all through a temporary local registry.
if [[ -n "${DRY_RUN:-}" ]]; then
    cargo publish --workspace --dry-run
fi
