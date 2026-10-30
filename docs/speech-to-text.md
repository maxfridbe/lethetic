# Desktop speech to text

Lethetic's Windows, macOS and Linux apps support local hold-to-talk dictation,
inspired by [sttme](https://github.com/maxfridbe/sttme). Microphone capture uses
CPAL and transcription runs inside the executable through whisper-rs/whisper.cpp.
The model stays loaded between recordings.

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
needs neither Network access nor Tor. Browser-only and Android builds do not expose
this desktop feature.

## Development and validation

Build from `native/` so its Cargo configuration applies portable CPU settings and
uses whisper-rs's bundled bindings. A C++ compiler and CMake are required; Linux
also needs ALSA development headers (`libasound2-dev` on Debian/Ubuntu). The native
CI workflow builds Windows, macOS and Linux and runs a real speech fixture on each.
Whisper is a desktop-only dependency, excluded from the Android application graph.

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
smoke test on the actual desktop/OS.

The lifecycle and local inference approach follow sttme. License notices for
[sttme](licenses/STTME-MIT.txt), [whisper.cpp](licenses/WHISPER-CPP-MIT.txt),
[whisper-rs](licenses/WHISPER-RS-UNLICENSE.txt) and [CPAL](licenses/CPAL-APACHE-2.0.txt)
are included with desktop bundles.
