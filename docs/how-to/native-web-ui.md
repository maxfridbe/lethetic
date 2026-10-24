# Use the native web UI

Lethetic's Tauri application packages the **same web UI, Rust WASM conversation
engine, Pyodide and Dotnetdide runtimes** as the browser distribution. Windows,
macOS, Linux and Android have build targets. It does not run generated Python or
.NET code on the host or require a host Python interpreter/.NET SDK.

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
cargo tauri android build --debug --target aarch64 --apk
```

The debug APK is installable for testing. Production APK/AAB builds require
[Android signing](https://v2.tauri.app/distribute/sign/android/). Generated Android
projects and native build outputs are ignored; the checked-in host plugin supplies
the foreground service and directory picker. The **Native Web UI** GitHub workflow
builds Windows NSIS, macOS DMG, Linux DEB/AppImage and an Android debug APK as
artifacts. It can also be run manually.

Before distributing, test real-directory import/write/conflict/revoke, untrusted
iframe IPC rejection, background streaming, stop/error cleanup, app closing,
rotation, Android notification behavior, and runtime initialization on real
Windows/macOS/Linux/Android devices. Compilation alone cannot verify webview or
OEM power behavior.
