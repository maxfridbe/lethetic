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

cargo test -p lethetic-wasm --locked
cargo build -p lethetic-wasm --target wasm32-unknown-unknown --release --locked
stage="$(mktemp -d "$ROOT/target/browser-build.XXXXXX")"
tests="$(mktemp -d "$ROOT/target/browser-tests.XXXXXX")"
trap 'rm -rf -- "$stage" "$tests"' EXIT
wasm-bindgen target/wasm32-unknown-unknown/release/lethetic_wasm.wasm \
  --target web --out-dir "$stage/wasm" --out-name lethetic_wasm
tsc --project browser/tsconfig.json --outDir "$stage" --pretty false
cp -R browser/static/. "$stage/"
cp integration.md "$stage/integration.md"
touch "$stage/.nojekyll"

tsc --project browser/tests/tsconfig.json --outDir "$tests" --pretty false
cp -R "$stage/wasm" "$tests/wasm"
for test in "$tests/tests/"*.test.js; do
  node --experimental-default-type=module "$test"
done

# Stage atomically only after runtime, TypeScript, and SDK tests pass.
python3 scripts/package_browser.py "$stage"
rm -rf target/browser
mv "$stage" target/browser
printf 'Built Pages site and embedding SDK: target/browser\n'
