#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'
export LC_ALL=C
export PYTHONDONTWRITEBYTECODE=1

readonly ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
readonly BUILD="$ROOT/build-web.sh"
readonly VERIFY="$ROOT/web/static/build-support/verify-web-assets.py"
readonly SOURCE_LINE_CHECK="$ROOT/scripts/check_source_lines.py"
readonly SOURCE_LINE_TEST="$ROOT/scripts/test_check_source_lines.py"
readonly APPROVAL_FLOW_TEST="$ROOT/web/static/build-support/approval-flow.test.mjs"
readonly APP_LAYOUT_STATUS_TEST="$ROOT/web/static/build-support/app-layout-status.test.mjs"
readonly CHAT_FOLLOW_TEST="$ROOT/web/static/build-support/chat-follow.test.mjs"
readonly JSON_RENDERING_TEST="$ROOT/web/static/build-support/json-rendering.test.mjs"
readonly MARKDOWN_RENDERING_TEST="$ROOT/web/static/build-support/markdown-rendering.test.mjs"
readonly TRANSPORT_AUTH_TEST="$ROOT/web/static/build-support/transport-auth.test.mjs"

temporary="$(mktemp -d)"
trap 'rm -rf -- "$temporary"' EXIT

bash -n "$BUILD"
bash -n "$ROOT/web/static/build-support/check-pipeline.sh"
python3 "$SOURCE_LINE_TEST"
python3 "$SOURCE_LINE_CHECK"
node --check "$APPROVAL_FLOW_TEST"
node --check "$APP_LAYOUT_STATUS_TEST"
node --check "$CHAT_FOLLOW_TEST"
node --check "$JSON_RENDERING_TEST"
node --check "$MARKDOWN_RENDERING_TEST"
node --check "$TRANSPORT_AUTH_TEST"
python3 - "$VERIFY" <<'PY'
import ast
from pathlib import Path
import sys
ast.parse(Path(sys.argv[1]).read_text(encoding="utf-8"), filename=sys.argv[1])
PY
(cd "$temporary" && python3 "$VERIFY" self-check)

(cd "$temporary" && "$BUILD" --check)

if ! find "$ROOT/web/src" -type f \( -name '*.ts' -o -name '*.tsx' \) \
    ! -path "$ROOT/web/src/generated/*" ! -name '*.d.ts' -print -quit | grep -q .; then
    set +e
    output="$(cd "$temporary" && "$BUILD" 2>&1)"
    status=$?
    set -e
    [[ $status -ne 0 ]] || {
        printf 'check-pipeline.sh: source-less normal build unexpectedly succeeded\n' >&2
        exit 1
    }
    case "$output" in
        *"web SPA sources are missing"*) ;;
        *)
            printf 'check-pipeline.sh: normal build did not report missing SPA sources\n%s\n' "$output" >&2
            exit 1
            ;;
    esac
fi

printf 'web pipeline checks passed\n'
