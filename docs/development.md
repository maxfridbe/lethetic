# Build and validate Lethetic

[Back to README](../README.md) · [Shared-core architecture](browser-parity.md)

Run these commands from the repository root. Native and browser behavior both
need coverage when changing shared Rust code.

## Native baseline

```bash
cargo build --release --bin lethetic
cargo test --lib
cargo test --bin lethetic
cargo test --test test_service_signals -- --nocapture
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

See the [native test reference](native-reference.md#testing) for container,
lifecycle, provider, and diagnostic checks. Native .NET tests need the container
image described in [native .NET verification](dotnet-native.md#verification).

## Browser and WASM

Install the prerequisites in the [browser build reference](../integration.md#build-and-test),
then run:

```bash
./build-browser.sh
CHROME_BIN=/path/to/chrome python3 scripts/test_browser.py
```

The build checks portable Rust, WASM, TypeScript, and the SDK. Browser acceptance
checks use a local mock provider; the reference describes optional real-runtime
coverage. Live provider checks can incur API charges. Supply keys through private
environment variables rather than fixtures, logs, screenshots, or committed files.

See [Pages and releases](../integration.md#github-pages-and-releases) for deployment
and release artifacts. Build outputs belong under `target/`, outside version control.
