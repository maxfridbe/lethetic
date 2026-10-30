# Native speech to text

Lethetic's Windows, macOS, Linux and Android apps support local hold-to-talk dictation,
inspired by [sttme](https://github.com/maxfridbe/sttme). Microphone capture uses
CPAL (AAudio on Android) and transcription runs inside the executable through whisper-rs/whisper.cpp.
The model stays loaded between recordings.

Capture incorporates the [sttme 0.1.1 microphone fixes](https://github.com/maxfridbe/sttme/commit/6928497):
DC-bias filtering, startup-pop suppression and a short fade-in. The level meter
uses speech energy and peaks, retains brief peaks between UI updates, and avoids
false zero readings when no new audio packet has arrived. Lethetic uses direct
CPAL capture, so sttme's `pw-record` pipe buffering workaround is unnecessary here.

1. Open the command palette and choose **Speech to text · hold Send**.
2. Choose **Tiny** (~75 MiB, faster) or **Base** (~142 MiB, more accurate), then
   **Download / load model**. Downloads come from the official
   `ggerganov/whisper.cpp` model repository and are checked against pinned SHA-256
   hashes. Downloading a model requires connectivity once; subsequent use is offline.
3. Hold **Send** for at least 300 ms. Speak when it says **Listening…**; the level
   meter shows microphone input. Release to transcribe and append the result to
   your existing draft. Review the text, then click **Send** to submit it.

A short click retains the usual Send behavior. You can also focus Send and hold
Space. Escape, the Cancel button, window blur, session changes and closing the UI
cancel dictation. Releasing never submits a prompt. Late results from cancelled
recordings are discarded. Each recording is limited to two minutes; inference has
a three-minute abort deadline. A missing or denied microphone produces a visible
error without sending or clearing the draft.

The default system microphone is used. Select another input in OS sound settings.
macOS prompts for microphone access; if that prompt moves focus away, approve it
and hold Send again. On Windows, enable microphone access for desktop apps in
Privacy settings. Linux uses the system's default ALSA input (normally routed by
PipeWire/PulseAudio). No global keyboard hook, virtual keyboard, shell helper,
Python installation or sttme daemon is required.

On Android 8.0+ (ARM64), Tiny is the default model. Setup requests Microphone
permission when you choose **Download / load model**. Grant it and return to
Lethetic before holding Send. If denied, chat still works; enable Microphone
in Android app settings or retry setup. Permission is checked again for every
recording. App switching, locking the screen and destroying the Activity cancel
recording and inference through a native lifecycle hook, even if the WebView is
paused. No background microphone service or Android/cloud speech service is used.

Language is detected automatically, or enter a Whisper language code such as `en`,
`es`, `fr` or `de` before loading the model. Turn off **Enable hold-to-talk** to
restore a plain Send button. Settings contain only enabled/model/language values;
they are independent of personas, model-provider keys and runtime permissions.

Audio stays in memory on this device. It is not uploaded, saved as a recording,
or added to debug bundles. Whisper's internal logging is disabled to avoid leaking
speech into process logs. Once you click Send, the transcript becomes normal prompt
text and follows the session's usual history and provider behavior. Models live in
the native application's local-data `whisper` folder. Explicit model downloads use
HTTPS separately from the code runtime's Direct/Tor permission; dictation itself
needs neither Network access nor Tor. Browser-only builds do not expose this native feature.

## Development and validation

Build from `native/` so its Cargo configuration applies portable CPU settings and
generates bindings for the target ABI. A C++ compiler, CMake and libclang are required; Linux
also needs ALSA development headers (`libasound2-dev` and `libclang-dev` on Debian/Ubuntu).
Do not set `WHISPER_DONT_GENERATE_BINDINGS`: upstream bundled Linux bindings do not
match Windows types. Android uses the NDK sysroot for both CMake and bindgen. The native
CI workflow builds Windows, macOS and Linux and runs a real speech fixture on each.
Android builds compile the same engine for ARM64 with the NDK CMake toolchain
and package its matching `libc++_shared.so`; APK validation rejects missing libraries.
The Android minimum is API 26 because CPAL uses AAudio. See the
[CPAL platform requirements](https://docs.rs/crate/cpal/0.18.2/source/README.md) and
[Android runtime permission guidance](https://developer.android.com/training/permissions/requesting).

```sh
cd native
cargo test -p lethetic-dictation --locked
python ../scripts/test_dictation.py
```

The second command downloads checksum-verified public test audio and a Tiny model,
then checks offline recognition, silence handling and cancellation during inference.
It never opens your microphone. Browser acceptance tests use a fake native audio
bridge to check quick clicks, holds, release during startup, error handling, Escape,
window blur and discarded late results. Hardware microphone access still needs a
smoke test on the actual device/OS. Android should also be checked with permission denied,
revoked, the system microphone privacy toggle off, and while backgrounding or locking
the device during capture.

The lifecycle and local inference approach follow sttme. License notices for
[sttme](licenses/STTME-MIT.txt), [whisper.cpp](licenses/WHISPER-CPP-MIT.txt),
[whisper-rs](licenses/WHISPER-RS-UNLICENSE.txt) and [CPAL](licenses/CPAL-APACHE-2.0.txt)
are included with native bundles.
