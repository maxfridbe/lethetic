#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'
umask 022
export LC_ALL=C
export PYTHONDONTWRITEBYTECODE=1

readonly REQUIRED_TSC_VERSION="7.0.2"
readonly ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly WEB="$ROOT/web"
readonly DIST="$WEB/dist"
readonly TSCONFIG="$WEB/tsconfig.json"
readonly VERIFY="$WEB/static/build-support/verify-web-assets.py"
readonly APPROVAL_FLOW_TEST="$WEB/static/build-support/approval-flow.test.mjs"
readonly APP_LAYOUT_STATUS_TEST="$WEB/static/build-support/app-layout-status.test.mjs"
readonly CHAT_FOLLOW_TEST="$WEB/static/build-support/chat-follow.test.mjs"
readonly JSON_RENDERING_TEST="$WEB/static/build-support/json-rendering.test.mjs"
readonly MARKDOWN_RENDERING_TEST="$WEB/static/build-support/markdown-rendering.test.mjs"
readonly TRANSPORT_AUTH_TEST="$WEB/static/build-support/transport-auth.test.mjs"
readonly FILES_PANE_TEST="$WEB/static/build-support/files-pane.test.mjs"

usage() {
    cat <<'USAGE'
Usage: ./build-web.sh [--check]

Build the browser-native TypeScript SPA into web/dist.
  --check  validate tools, configuration, contracts, sources, and assets
           without generating contracts or writing web/dist
USAGE
}

die() {
    printf 'build-web.sh: %s\n' "$*" >&2
    exit 1
}

mode="build"
case "${1-}" in
    "") ;;
    --check) mode="check" ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: ${1}" ;;
esac
[[ $# -le 1 ]] || { usage >&2; die "too many arguments"; }

command -v python3 >/dev/null 2>&1 || die "python3 is required for dependency-free asset validation"
command -v cargo >/dev/null 2>&1 || die "cargo is required to generate the Rust/TypeScript contracts"
command -v node >/dev/null 2>&1 || die "Node.js is required for dependency-free browser transport tests"
tsc_bin="$(type -P tsc || true)"
[[ -n "$tsc_bin" ]] || die "global tsc $REQUIRED_TSC_VERSION is required on PATH"
case "$tsc_bin" in
    "$ROOT"/*) die "refusing project-local tsc; install global tsc $REQUIRED_TSC_VERSION" ;;
esac
tsc_version="$($tsc_bin --version 2>&1)" || die "could not run global tsc"
[[ "$tsc_version" == "Version $REQUIRED_TSC_VERSION" ]] || \
    die "global tsc $REQUIRED_TSC_VERSION is required (found: $tsc_version)"
[[ ! -L "$DIST" ]] || die "refusing symlink web/dist"

run_browser_tests() {
    node --experimental-default-type=module "$TRANSPORT_AUTH_TEST" "$1"
    node --experimental-default-type=module "$APPROVAL_FLOW_TEST" "$1"
    node --experimental-default-type=module "$APP_LAYOUT_STATUS_TEST" "$1"
    node --experimental-default-type=module "$CHAT_FOLLOW_TEST" "$1"
    node --experimental-default-type=module "$JSON_RENDERING_TEST" "$1"
    node --experimental-default-type=module "$MARKDOWN_RENDERING_TEST" "$1"
    node --experimental-default-type=module "$FILES_PANE_TEST" "$1"
}

python3 "$VERIFY" self-check

if [[ "$mode" == "check" ]]; then
    python3 "$VERIFY" check --require-application
    CARGO_NET_OFFLINE=true cargo run \
        --manifest-path "$ROOT/Cargo.toml" \
        --locked --offline --quiet --bin generate_web_contracts -- \
        --check --root "$ROOT"
    check_stage="$(mktemp -d "$WEB/.dist.check.XXXXXX")"
    trap 'rm -rf -- "$check_stage"' EXIT
    "$tsc_bin" --project "$TSCONFIG" --outDir "$check_stage" --pretty false
    python3 "$VERIFY" copy-assets "$check_stage"
    run_browser_tests "$check_stage"
    python3 "$VERIFY" compare-dist "$check_stage"
    python3 "$ROOT/scripts/generate_web_asset_manifest.py"
    rm -rf -- "$check_stage"
    trap - EXIT
    printf 'web build check passed (tsc %s; web/dist matches sources)\n' "$REQUIRED_TSC_VERSION"
    exit 0
fi

# Fail before generating anything when task #61's application sources are absent.
python3 "$VERIFY" check --require-application
CARGO_NET_OFFLINE=true cargo run \
    --manifest-path "$ROOT/Cargo.toml" \
    --locked --offline --quiet --bin generate_web_contracts -- \
    --root "$ROOT"

stage=""
previous=""
cleanup() {
    status=$?
    if [[ -n "$stage" && -e "$stage" ]]; then
        rm -rf -- "$stage"
    fi
    if [[ -n "$previous" && -e "$previous" ]]; then
        if [[ ! -e "$DIST" ]]; then
            mv -- "$previous" "$DIST" || true
        else
            rm -rf -- "$previous"
        fi
    fi
    exit "$status"
}
trap cleanup EXIT

stage="$(mktemp -d "$WEB/.dist.tmp.XXXXXX")"
"$tsc_bin" --project "$TSCONFIG" --outDir "$stage" --pretty false
python3 "$VERIFY" copy-assets "$stage"
run_browser_tests "$stage"
python3 "$VERIFY" check-dist "$stage"

if [[ -e "$DIST" ]]; then
    [[ -d "$DIST" && ! -L "$DIST" ]] || die "web/dist exists but is not a regular directory"
    previous="$(mktemp -d "$WEB/.dist.previous.XXXXXX")"
    rmdir -- "$previous"
    mv -- "$DIST" "$previous"
fi
mv -- "$stage" "$DIST"
stage=""
if [[ -n "$previous" ]]; then
    rm -rf -- "$previous"
    previous=""
fi
# Keep every manifested resource an explicit compiler dependency.
python3 "$ROOT/scripts/generate_web_asset_manifest.py" --write
# Asset mtimes are normalized for deterministic output, so explicitly invalidate
# Cargo's mtime-based include_dir/include_bytes dependency check after a rebuild.
touch -- "$ROOT/src/wfe/assets.rs"
printf 'built deterministic web distribution at %s\n' "$DIST"
