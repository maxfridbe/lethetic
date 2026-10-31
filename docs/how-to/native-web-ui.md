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

The Files dock toggle supports both **left** and **top** layouts on phones,
tablets and desktops. In left mode, the file list sits above the preview beside
the conversation; drag the divider to resize it. Monaco syntax highlighting and
its worker are bundled locally and do not need Network access or a CDN.

The top status line shows the runtime (**PY** or **.NET**), virtual or granted
directory, and code network policy (**TOR only** when selected). It describes
the session permission, not Tor circuit readiness or model-provider routing.
The model name is pinned in the bottom stats bar; existing stat selections remain.

## Save downloads

**Download File**, snippet **Download**, **Download all (.zip)**, and the session
**Database (.db)** / **Export debug (.tar.gz)** buttons open the operating system's
Save dialog in the app. On Android, choose Downloads or another document provider
and confirm Save. Cancelling leaves the session unchanged. Errors appear in the
Files or Sessions status instead of reporting a successful download.

Saving one file does not grant a workspace folder or enable Network access.
Binary data is staged in 256 KiB chunks and copied to the selected destination;
outputs up to 512 MiB are supported by the native save bridge. Existing workspace
and preview limits still apply. Staging files are removed on success, cancellation
or transfer failure. Browser-only mode continues to use normal browser downloads.

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

- Only the selected tree is available. No shell or arbitrary host-filesystem
  command is exposed to generated code. Network access has its own permission.
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
grants, workspace synchronization, user-selected download destinations, revocable runtime HTTP access, explicit loopback site forwarding and response keep-awake. No remote
origin has an IPC grant. This is the browser runtime's isolation model with a
narrow directory bridge, not Podman/container isolation or a defense against a
compromised operating system/document provider.

## Hold Send to dictate

Windows, macOS, Linux and Android builds include local Whisper speech to text. Open
**Speech to text · hold Send** in the command palette and download/load a model.
Hold Send to record, release to append the transcript to your draft, then review
and click Send. Escape or leaving the window cancels. Audio stays on your device;
only the model needs a one-time download. Android requires 8.0+ (ARM64), defaults
to Tiny, and requests Microphone permission during setup. See [setup, permissions and testing](../speech-to-text.md).

## Network access without proxy setup

In Connections → Runtime & security, choose **Code network access**:

- **Off** blocks runtime HTTP.
- **Direct network** uses the native HTTP client without proxy setup or destination CORS.
- **Tor only · embedded Arti** sends runtime HTTP through the Tor Project's Rust
  [Arti client](https://docs.rs/arti-client/0.47.0/arti_client/), bundled into the
  executable. It needs no Tor daemon, SOCKS configuration or host installation.

Python `await pyodide.http.pyfetch(...)`, C# `HttpClient`, and their site handlers
use the selected route. Tor-only mode passes destination names to Tor instead of
resolving them locally, validates HTTPS certificates, and never retries through
direct HTTP. Public hostnames and v3 `.onion` addresses are supported; IP literals,
localhost and local-network names are blocked. Tor bootstrap happens before code
execution, with a 60-second deadline. A failed connection reports an error; retry
or explicitly change the policy. Arti caches network state in the application's
private data directory. State survives app restarts; active grants do not.

The choice is saved per session in `sessions.network_mode`. Changing it resets
code variables and running sites while retaining chat and files. Opening a Tor-only
session in a normal browser blocks runtime HTTP because Arti is unavailable;
it does not silently downgrade the permission. The browser distribution retains
its direct/relay selector. Saved browser relay credentials are ignored by native
runtimes and remain available when the session is opened in a browser.

**Diagnosing failures:** a destination HTTP 403 means rejection, not proof that
Network access is disabled, rate limiting occurred, or Tor caused the block. A
relay HTTP 502 means forwarding failed; the tool now preserves a bounded,
sanitized explanation such as a DNS, connection, TLS or timeout failure. Requests
do not carry browser cookies or execute website JavaScript. Use an authorized API
or another source when a site returns a challenge; Tor-only requests never retry
over direct HTTP. Save successful search results before issuing further requests.

**Sessions → Export debug (.tar.gz)** includes `network-diagnostics.json` for
native Python/.NET relay requests: hostname, method, direct/Tor mode, start time,
duration, relay status, target status and sanitized transport errors. Paths,
queries, headers, request/response bodies and credentials are not recorded. The
capture retains the last 200 completed requests per session, across runtime resets,
for the last 16 sessions used during this app process. It is in memory only and
resets on app exit. Omission counts identify evicted requests. Direct browser fetch,
model requests and requests still running are outside this capture; missing capture
is reported explicitly. Model streams remain in `provider-traces.jsonl`.

Tor-only mode disables **direct NuGet downloads**. Bundled libraries still work;
additional Python downloads use the runtime's HTTP relay. Forwarded site pages
cannot fetch external URLs directly in Tor-only mode: page JavaScript should call
a local Python/.NET route, which then uses Tor. Use relative site URLs.

This setting governs cooperative runtime HTTP, not every connection from the app
or OS. Model-provider requests and exact bundled runtime/package asset downloads
keep their separate routes. Worker JavaScript interop is not a hostile-code
sandbox. Opening a site in another browser does not turn that browser into Tor
Browser. Bridges/pluggable transports and application-wide VPN routing are not
part of this option.

The app supplies a temporary, authenticated loopback HTTP route to each enabled
runtime. Disabling or changing network access, stopping/resetting the runtime, switching
sessions or closing the app revokes it and cancels pending requests. Reloading the
main page revokes prior grants. The main UI's native capability is never passed
to code workers or previews. Direct native HTTP can reach public and local-network
services; Tor-only access blocks local targets and has no direct fallback; it does not grant filesystem or shell access.

GET, HEAD, OPTIONS, POST, PUT, PATCH and DELETE support explicit API authorization
headers and binary bodies up to 8 MiB in each direction. Requests time out after
60 seconds. Browser cookies and automatic redirects are not forwarded; raw sockets
and WebSocket proxying are not provided. NuGet and bundled runtime package downloads
keep their existing separate permissions/routes. Model connections are unchanged.

## Test a site in the device browser

1. Enable **Interactive sites**, then run your Python or C# site from Files.
2. Open **Sites**, select its virtual port, and find **Open in the device browser**.
3. Leave **Real port** at `0` to choose an available port, or enter `1024–65535`.
4. Press **Forward & open browser**. The device browser opens a real
   `http://127.0.0.1:<port>/` site backed by the running session.

Relative API requests, scripts, ES modules, styles, images and forms use ordinary
HTTP. Use relative URLs: the runtime still identifies its own virtual port, and
its existing header/cookie restrictions still apply. Python/.NET handlers and
files remain inside their WASM runtime. This is a
loopback HTTP bridge, not a host Python/.NET server or a LAN/public listener.
Outbound runtime requests still require **Network access**; external requests from
page JavaScript also follow the browser's CORS rules. Without Network access,
the page's content policy permits local resources only.

Use **Open browser** again to authorize another tab/browser, and **Stop forwarding**
to close the real port while leaving the virtual site running. The launch URL
exchanges a temporary capability for an HttpOnly cookie before loading user content.
Closing the Sites dialog keeps forwarding active. Stopping the virtual site,
changing sessions/runtime/permissions, reloading or closing Lethetic revokes it.

Requests are serialized through the runtime, with 16 MiB request and 64 MiB
buffered response limits and a 60-second bridge deadline; shorter runtime deadlines
still apply. A busy runtime can return 503. WebSocket upgrades, TLS and server-sent
streaming are not supported by this bridge. External browser console logs stay in
that browser's developer tools; they are not captured by the iframe debugger.

Keep Lethetic running. On Android, explicit forwarding maintains the foreground
notification and WebView availability, but **does not acquire a CPU wake lock**.
The service has the same 30-minute lease ceiling as background turns; device sleep
or process termination can interrupt hosting. Browser-only builds cannot bind a
real TCP port and keep the existing sandboxed Sites preview.

## Android credentials and response audio

Saved connection and persona voice API keys persist between Android runs. They
are encrypted using Android Keystore in app-private, backup-excluded storage.
Existing keys still in the current tab migrate automatically; keys already lost
by an older build must be entered once and saved. Leave a key field blank to keep
its saved value. Clearing a key removes it from the vault. Normal signed upgrades
preserve the vault; uninstalling or clearing app data removes it. Session exports
and persona JSON never contain these keys. Storage/decryption failures are shown
rather than silently replacing the vault or falling back to plaintext.

**Play** beside a response starts native Android media playback. Its own foreground
media service fetches, caches and plays audio independently of the WebView, so
switching apps or locking the screen does not stop it. The media notification and
lock-screen controls provide Pause, Play and Stop. Audio focus loss and unplugging
headphones pause playback. Force-stop or OS process termination still ends playback;
it does not restart unexpectedly.

**Download audio** beside Play saves the completed cached WAV through Android's
file picker. No second synthesis request is needed. The private audio cache is
bounded to 128 MiB per response and 512 MiB total, evicting older recordings; it
contains audio, not endpoint credentials, and is omitted from session/debug exports.
Android may reclaim cached files. Response audio has its own CPU wake lease while
playing, released on pause/completion/stop; this is separate from model keep-awake.
See [persona voice settings](../../persona.md#streaming-voice-playback) for formats
and endpoint configuration.

## Background responses

For model work, keep-awake is acquired **only while awaiting or streaming a model response**. It
is released before tool execution, during retry backoff, at idle, on cancellation
and on error. Idle model sessions do not leave a background daemon running.

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

macOS builds require macOS 11 or later; this also supplies the C++ filesystem
runtime used by local Whisper dictation.

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
cp -R src-tauri/icons/android/. src-tauri/gen/android/app/src/main/res/
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
