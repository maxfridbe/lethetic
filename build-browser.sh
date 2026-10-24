#!/usr/bin/env bash
set -Eeuo pipefail
ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$ROOT"

for tool in cargo wasm-bindgen tsc node python3; do
  command -v "$tool" >/dev/null || { echo "Missing build tool: $tool" >&2; exit 1; }
done
[[ "$(wasm-bindgen --version)" == "wasm-bindgen 0.2.128" ]] || {
  echo 'Install matching bindings: cargo install wasm-bindgen-cli --version 0.2.128 --locked' >&2; exit 1;
}
[[ "$(tsc --version)" == "Version 7.0.2" ]] || {
  echo 'Install TypeScript 7.0.2 (npm install --global typescript@7.0.2)' >&2; exit 1;
}

cargo test -p lethetic --no-default-features --lib --locked
cargo test -p gemma-chat --no-default-features --lib --locked
cargo test -p lethetic-wasm --locked
python3 scripts/release_version.py --check
cargo build -p lethetic-wasm --target wasm32-unknown-unknown --release --locked
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s browser/tests -p 'test_python_*.py'
stage="$(mktemp -d "$ROOT/target/browser-build.XXXXXX")"
tests="$(mktemp -d "$ROOT/target/browser-tests.XXXXXX")"
trap 'rm -rf -- "$stage" "$tests"' EXIT
wasm-bindgen target/wasm32-unknown-unknown/release/lethetic_wasm.wasm \
  --target web --out-dir "$stage/browser/wasm" --out-name lethetic_wasm
tsc --project browser/tsconfig.json --outDir "$stage" --pretty false
cp -R web/lib "$stage/web/lib"
cp web/static/styles.css web/static/files.css "$stage/web/"
cp -R browser/static/. "$stage/"
cp integration.md "$stage/integration.md"
mkdir -p "$stage/docs" "$stage/src/storage"
cp src/storage/schema.sql "$stage/src/storage/schema.sql"
cp docs/browser-parity.md docs/storage.md "$stage/docs/"
touch "$stage/.nojekyll"
python3 scripts/fetch_dotnetdide.py "$stage"

tsc --project browser/tests/tsconfig.json --outDir "$tests" --pretty false
cp -R "$stage/browser/wasm" "$tests/browser/wasm"
cp -R web/lib "$tests/web/lib"
for test in "$tests/browser/tests/"*.test.js; do
  node --experimental-default-type=module "$test"
done

# Stage atomically only after runtime, TypeScript, and SDK tests pass.
python3 scripts/package_browser.py "$stage"
rm -rf target/browser
mv "$stage" target/browser
printf 'Built Pages site and embedding SDK: target/browser\n'
