# Use the native web UI

Lethetic's Tauri application packages the **same web UI, Rust WASM conversation
engine, Pyodide and Dotnetdide runtimes** as the browser distribution. Windows,
macOS, Linux and Android have build targets. It does not run generated Python or
.NET code on the host or require a host Python interpreter/.NET SDK.

Android reserves space for status/navigation bars, display cutouts and the
keyboard outside the WebView, including in landscape and fullscreen previews.
The launcher uses the current Lethetic emblem with an Android 13+ monochrome
layer for launchers with **Themed icons** enabled. The in-app emblem follows the
selected application theme. Regenerate packaged icons from `res/lethetic-mark.svg`
with `python3 scripts/generate_native_icons.py` (requires the Tauri CLI).

## Choose your workspace

The default is **Virtual directory**. Session files live in the application's
local session database and are synchronized into the selected WASM runtime.
Existing Network, NuGet and Sites permissions still apply. Model-provider requests
remain separate from code-network permission, just as in the browser.

In Files, select **Virtual · Open folder** to grant a real directory. The native
folder picker opens (Android uses the Storage Access Framework tree picker).
Opening a folder replaces the current session's virtual file snapshot with that
folder's contents. Save/export existing virtual files first if you want to keep
them separately. Completed code calls and file edits save changed files back to
the selected directory. Runtimes still see a virtual filesystem, not host paths.

- Only the selected tree is available. No shell, arbitrary host-filesystem or
  native HTTP command is exposed to generated code.
- Hidden paths (including `.env` and `.git`), symbolic links and special files are excluded.
  Desktop filesystem operations use directory capabilities that reject escapes.
- The browser limits still apply: 16 MiB/file, 64 MiB/workspace, 100 files and
  256 directories. Selecting an oversized directory fails explicitly.
- An outside edit detected when synchronizing conflicts with a pending write. Reopen the folder to
  reload external changes; there is no silent overwrite or filesystem watcher.
- Deleting a virtual file does **not** delete its real-directory counterpart.
- **Disconnect** returns to virtual mode and keeps the session's current files.
  Session changes and app exit revoke the grant; reopening a session does not
  silently regain host access.
- Desktop writes replace individual files atomically. Android document providers
  may not support atomic replacement. Failed writes keep the virtual copy and
  display an error; export it or retry after resolving the problem.

The main application is trusted. Sandboxed previews and code workers do not get
its per-launch native capability token. Tauri permissions are limited to folder
grants, workspace synchronization, revocation and response keep-awake. No remote
origin has an IPC grant. This is the browser runtime's isolation model with a
narrow directory bridge, not Podman/container isolation or a defense against a
compromised operating system/document provider.

## Background responses

Keep-awake is acquired **only while awaiting or streaming a model response**. It
is released before tool execution, during retry backoff, at idle, on cancellation
and on error. Closing the app does not leave a background daemon running.

Desktop builds request an OS idle-sleep inhibitor; the display can turn off.
macOS also disables webview background throttling where supported. Android uses
a visible foreground service during the user-initiated turn, resumes its WebView,
and holds a partial wake lock only while a response is pending. Keeping the
notification through tool execution allows later model rounds to continue
without attempting to start a new service from the background. Each Android lease has a 30-minute ceiling and is
not restarted automatically after process death. This feature is not an Android
store publication/signing setup; foreground-service declarations need review
when publishing to a store.

Forced sleep, closing the lid, OS memory pressure, Android force-stop and OEM
battery policies can still interrupt execution. A foreground service reduces
that risk; it does not make a WebView immortal. Long unattended work requiring
process lifetime independent of a UI should use the [CLI/remote mode](remote-control.md)
on an awake machine.

Browser and native timeout budgets exclude hidden-document time and scheduler
gaps over five seconds, rather than expiring immediately on resume. They do not
make a provider retain a dropped network connection. Browser screen wake locks
are best effort and work only while the document is visible.

The session is checkpointed before a request and after completed code work,
including transcript and file effects. If the process disappears, reopening the
session shows a recovery checkpoint. Review files and explicitly continue;
Lethetic never automatically replays an uncertain tool call. Variables reset
when runtimes restart. Debug exports include bounded page lifecycle events.

## Build without a Node dependency tree

The existing browser build uses the repository's TypeScript compiler and test
runner; the native shell adds Rust dependencies and Android Gradle dependencies,
not a package.json/npm application toolchain.

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for
your OS, Rust, the repository's browser tools, and the Tauri CLI:

```sh
cargo install tauri-cli --version 2.12.4 --locked
./build-browser.sh
python3 scripts/prepare_native.py
cd native
cargo test -p lethetic-directory --locked
cargo tauri build
```

The `native/` workspace is separate so the CLI/WASM builds do not acquire GUI
system dependencies. Bundles are written to `native/target/release/bundle/`.
Build on each target OS; Windows/macOS signing and macOS notarization need your
own certificates. Unsigned builds may trigger OS installation warnings.

For Android, install Java 21, Android SDK platform/build-tools 36 and NDK
29.0.14206865. Set `ANDROID_HOME` and `NDK_HOME`, then:

```sh
rustup target add aarch64-linux-android
cd native
cargo tauri android init --ci
cargo tauri android build --target aarch64 --apk
python3 ../scripts/android_release.py --sign
```

This builds the optimized release variant. The packaging script reads the signing
environment variables described below, rejects debuggable APKs and debug signing
certificates, aligns the APK, signs it, and verifies its signature and alignment.
It writes `native/android-release/com.TechHurts.Lethetic_v<version>.apk` (ARM64).
The download filename does not change the Android application ID. Generated Android
projects and native build outputs are ignored; the checked-in host plugin supplies
the foreground service and directory picker.

The **Native Web UI** GitHub workflow builds Windows NSIS, macOS DMG, Linux
DEB/AppImage and an Android release APK. Branch/PR builds produce an explicitly
**unsigned** Android Actions artifact for build verification; it is not installable
or published as a release download. Tag builds (`vyy.mmdd.##`) require the
release signing secrets below to publish Android. Missing secrets produce a visible
workflow warning and an unsigned Actions artifact; APK publication is skipped.
Invalid credentials fail signing. There is no debug-key fallback, and desktop,
CLI, and web/WASM downloads publish independently. Android's app version and download
filename are generated from the central `VERSION` file. See [release versioning](../releases.md).

Tags attach the desktop installers and their SHA-256 checksums to the same
[GitHub release](https://github.com/maxfridbe/lethetic/releases/latest) as the browser
ZIP. The signed Android installer is added independently when signing is configured.
After adding signing secrets, rerun this workflow against the same release tag to
attach the APK without changing versions. Look under **Assets** on that release; an Actions artifact is an intermediate
build download. Manually running the workflow against a release tag also signs
and publishes; running it against a branch produces unsigned Android output.

### Android release signing setup

Use a persistent release keystore, following the
[Android signing guide](https://v2.tauri.app/distribute/sign/android/). To create one,
run this interactively outside the repository; `keytool` prompts for the password:

```sh
keytool -genkeypair -keystore /secure/path/lethetic-release.jks -storetype JKS \
  -alias lethetic -keyalg RSA -keysize 3072 -validity 10000
```

Back up the keystore, alias, and passwords securely. Reuse the same key for future
releases so installed apps can be updated. Add these repository **Actions secrets**
under Settings → Secrets and variables → Actions:

| Secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | Base64 encoding of the keystore file |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore password |
| `ANDROID_KEY_ALIAS` | Signing alias, e.g. `lethetic` |
| `ANDROID_KEY_PASSWORD` | Password for that alias |

With an authenticated GitHub CLI, upload the keystore without printing its contents:

```sh
base64 < /secure/path/lethetic-release.jks | gh secret set ANDROID_KEYSTORE_BASE64 --repo maxfridbe/lethetic
gh secret set ANDROID_KEYSTORE_PASSWORD --repo maxfridbe/lethetic
gh secret set ANDROID_KEY_ALIAS --repo maxfridbe/lethetic
gh secret set ANDROID_KEY_PASSWORD --repo maxfridbe/lethetic
```

For local signing, supply the same four environment variables securely. The helper
uses a temporary keystore outside artifacts and build caches, and removes it on
success or failure. It does not save passwords in generated Gradle files.

The old `browser-v0.8.0` APK was debug-signed. Switching to a release key requires
exporting any sessions you want to preserve, uninstalling that debug app, then
installing the release APK; Android will not accept it as an in-place update with
a different signing certificate.

Before distributing, test real-directory import/write/conflict/revoke, untrusted
iframe IPC rejection, background streaming, stop/error cleanup, app closing,
rotation, Android notification behavior, and runtime initialization on real
Windows/macOS/Linux/Android devices. Compilation alone cannot verify webview or
OEM power behavior.
