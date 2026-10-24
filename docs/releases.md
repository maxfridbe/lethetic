# Releases and central versioning

Every Lethetic distribution shares the root [`VERSION`](../VERSION) file. Its
format is **`yy.mmdd.##`**, using UTC and a two-digit daily sequence starting at
`01`. For example, `26.1024.01` is the first release on October 24, 2026;
`26.1024.02` is the second. The sequence resets the next day and stops at `99`.

The Git tag is `v26.1024.01` and the release title is **Lethetic v26.1024.01**.
CLI, web/WASM, embedding, Windows, macOS, Linux and Android downloads all belong
to that release. There are no new `browser-v*` releases. Existing tags and their
historical assets remain unchanged.

## Downloads

For version `26.1024.01`, the release assets are:

| Distribution | Asset |
| --- | --- |
| Web UI, WASM engine and embedding SDK | `lethetic-web-wasm_v26.1024.01.zip` |
| Linux x86-64 CLI and runtime helper | `lethetic-cli-linux-x86_64_v26.1024.01.tar.gz` |
| Windows x86-64 native UI | `lethetic-windows-x86_64_v26.1024.01.exe` |
| macOS Apple Silicon native UI | `lethetic-macos-arm64_v26.1024.01.dmg` |
| Linux x86-64 native UI | `lethetic-linux-x86_64_v26.1024.01.deb` / `.AppImage` |
| Android ARM64 native UI | `com.TechHurts.Lethetic_v26.1024.01.apk` |

Each download has a `.sha256` file. Workflows attach their assets to the same
release as each finishes. Android requires the persistent
[release signing secrets](how-to/native-web-ui.md#android-release-signing-setup);
unsigned or debug APKs cannot be published by the release job. Missing Android
secrets skip APK publication with a workflow warning; they do not block the
CLI, web/WASM, or desktop downloads. After configuring the secrets, rerun the
Native Web UI workflow on the same tag to add its signed APK. The APK filename
does not change the installed application's identifier.

## Prepare a release

```sh
# Use --set for an explicit version, or --next for today's next UTC sequence.
python3 scripts/release_version.py --set 26.1024.01
# python3 scripts/release_version.py --next
python3 scripts/release_version.py --check
python3 scripts/test_release_version.py
python3 scripts/test_android_release.py
```

The helper updates `VERSION`, all six first-party Cargo package versions, both
lockfiles, and the Tauri configuration together. Review and commit those changes,
run the relevant build checks, then tag that commit with `v` followed by `VERSION`
and push the commit and tag. Never edit the generated package versions separately.
CI rejects mismatched tags or stale package metadata. A normal branch push deploys
Pages and tests native builds, without publishing a release.

`--next` advances the checked-in counter; it does not fetch remote tags or create
a Git tag. Synchronize your branch/tags first and use a unique version per release.
Release cleanup retains the newest **30 published releases**, keeping Git tags
and draft releases.

## Package version encodings

Release titles, filenames, the web manifest and bundled `VERSION` retain the
padded calendar version. Cargo, npm metadata and Tauri require numeric SemVer,
so their generated version omits leading zeros: `26.0102.03` becomes `26.102.3`.
The browser package also records the full value in `letheticVersion`.

Android's generated `versionCode` concatenates the date and sequence, e.g.
`26010203`, making it increase across releases. macOS's generated `CFBundleVersion`
uses `yymm.dd.sequence`, e.g. `2601.2.3`, to fit its numeric component limits.
These are deterministic encodings of the same central version, not independent
release counters. See the [Tauri package version configuration](https://v2.tauri.app/reference/config/#version).
