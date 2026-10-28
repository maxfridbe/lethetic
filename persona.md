# Personas

Open the command palette (**Ctrl+P**, or the top-left logo) and select **Personas**.
The palette also has **Import persona JSON** and **Export persona JSON** shortcuts.
The same screen is available in browser-only, native app, and remote-control modes.

Choose **Lethetic — Sassy** (the default), **Skippy the Magnificent**, **Mr. Meeseeks**,
**HAL9000**, or **Space GHOST**. Character presets include recognizable catchphrases alongside original quips and illustrations.
Skippy uses operatic arrogance and playful monkey jokes; HAL uses measured, formal
speech without contractions or exclamation marks; Meeseeks starts enthusiastic
and can become comically frantic as a task encounters setbacks. These traits
apply to narration, not correctness, permissions, or task decisions.
Audio presets use generated tones, not actor recordings or cloned voices.

The screen lets you switch personas, inspect their quips and system instructions,
import/export JSON, and independently enable:

- **Narration in system prompt:** applies the selected persona to the next message.
- **Thinking quips:** replaces the rotating gradient status text.
- **Animated SVG companion:** a transparent image above the prompt bar that reacts to events.
- **Event sounds:** optional tones or embedded audio. Off by default.

**Enable persona features** switches these features and speech playback off without deleting the selection
or individual preferences. Display changes apply immediately, including during a
turn. Narration changes apply on the next submitted message, not halfway through
a response. Reduced-motion preferences suppress SVG animation and quip rotation.

## Streaming voice playback

In **Personas → Speech playback**, choose **OpenAI-compatible** or
**ElevenLabs-compatible**, enter the full endpoint URL, model, voice name/ID and
API key, enable response Play buttons, then **Save voice**. Settings belong to the
selected persona; switching personas selects its voice. **Test voice** plays a
short sample. Beside each assistant response, **Play** reads that response (including
any Markdown/code text) and becomes a **square Stop** control while fetching or
playing audio. Only an explicit Play/Test click sends text; event sounds remain
independent. During a streaming model response, Play reads the text present when
clicked, not later additions. Voices are AI-generated.

In **PCM** mode, audio starts as bytes arrive, before synthesis finishes.
For an OpenAI-compatible server that returns files, choose **Audio format → WAV**;
playback starts after each complete file downloads and decodes. **Stop** cancels the
request and queued audio immediately; starting another response replaces playback.
You can also stop playback from the Personas voice panel when a response has
scrolled out of view. Switching personas/sessions, disabling persona features, or
unmounting the UI stops playback. Long responses are split into requests of at most
4,000 characters without dropping text. PCM playback maintains a small audio queue;
WAV mode finishes each recording before downloading the next file.
A request with no network progress for 60 seconds stops with a retryable error.

Voice configuration is included in persona JSON, but **API keys stay only in
memory in the current UI instance**. They are not written to localStorage, session
databases, debug reports, or exports; re-enter them after a reload. A saved key is
shown as present, and an empty key field keeps it. **Clear key** removes it. Keys
are scoped to the persona, API style and exact endpoint; changing an endpoint never
reuses another endpoint's key. Keyless local speech servers are supported.

The endpoint must allow browser CORS requests. PCM mode expects **raw signed
16-bit, little-endian, mono PCM at 24 kHz**; WAV mode expects a binary RIFF/WAVE
file in a browser-decodable codec (maximum 64 MiB per request). WAV headers supply
the sample rate and channel count. HTTPS is required except for loopback HTTP.
Redirects are rejected so authentication is never forwarded to another endpoint.
This works in the shared browser, native-app WebView and remote-control UI; the
request originates from the UI, not from a Python/.NET runspace. Runspace network
permissions do not grant or control speech access. Clicking Play sends the response
to the chosen provider and may incur its normal charges. JSON file links, base64,
and MP3 responses are not supported. See [Speech endpoint compatibility](docs/speech-endpoints.md)
for server request/response examples, PCM streaming, CORS and local-server setup.

Add an optional `speech` object to an exported persona:

```json
"speech": {
  "enabled": true,
  "provider": "openai",
  "endpoint": "https://api.openai.com/v1/audio/speech",
  "model": "gpt-4o-mini-tts",
  "voice": "coral"
}
```

Or use ElevenLabs (replace `voice` with an ID available to your account):

```json
"speech": {
  "enabled": true,
  "provider": "elevenlabs",
  "endpoint": "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream",
  "model": "eleven_multilingual_v2",
  "voice": "JBFqnCBsd6RMkjVDRZzb"
}
```

Set the optional persona field `speech.format` to `"wav"` for WAV file endpoints,
or `"pcm"` (the default when absent) for streaming. WAV is available with the
OpenAI API style; ElevenLabs continues to request PCM.

OpenAI requests use Bearer authentication and `response_format: "pcm"` or `"wav"`.
ElevenLabs requests use `xi-api-key`, `model_id` and
`output_format=pcm_24000`; `{voice_id}` is substituted with the encoded voice ID.
See the [OpenAI speech guide](https://developers.openai.com/api/docs/guides/text-to-speech)
and [ElevenLabs streaming API](https://elevenlabs.io/docs/api-reference/text-to-speech/stream)
for provider-specific models, voices and account requirements. Endpoint configuration
and credentials are never added to the persona's system prompt.

## Narration and session state

The generated system section begins `### Persona — narration only`. It explicitly
limits personality to brief conversational framing and progress narration. Task
accuracy, tool choices, code, files, quotations, structured data, requested formats,
and the user's requested tone take precedence. A persona does not grant permissions
or change the code runtime, connection, files, or session identity.

On submission, the UI sends the section separately from the user's message. Both
local WASM and the remote CLI replace only the managed section at the end of the
system prompt; they preserve the base prompt and conversation. Disabling narration
sends an empty section and removes the previous one on the next submission. A
running turn keeps the section it started with. The effective section is saved
with the session's system prompt, so exports/debug reports include it. Ordinary
CLI requests and older callers that omit the optional `persona` command field
leave the existing system prompt intact.

Selection, custom personas, and feature switches are device/browser preferences,
under `lethetic.personas.v1`; they apply across sessions in that UI. Importing a
session does not import its artwork/audio library. The next UI submission applies
that device's currently selected persona. The Personas screen shows the section
that will be applied; the system prompt editor shows the last applied section.

## Make a persona

Export a built-in from **Personas → Export JSON**, edit the file, and use
**Import JSON**. Import selects the validated persona. Reusing an ID replaces that
custom definition; using a built-in ID creates a local override. **Reset / delete
custom** removes the override (restoring the built-in) or deletes a custom persona.
Malformed imports leave the current selection and library unchanged.

A minimal file:

```json
{
  "version": 1,
  "id": "patient-engineer",
  "name": "Patient Engineer",
  "description": "Measured explanations and dry technical humor.",
  "narration": "Be calm, concise, and gently witty. Explain progress plainly.",
  "quips": [
    "Checking the assumptions before they become architecture…",
    "Giving the edge cases a comfortable place to sit…"
  ],
  "avatar": {
    "svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><circle cx=\"50\" cy=\"50\" r=\"30\" fill=\"#38bdf8\"/><path d=\"M35 55Q50 70 65 55\" fill=\"none\" stroke=\"#0f172a\" stroke-width=\"4\"/></svg>",
    "events": {
      "when_user_submits_prompt": "bounce",
      "when_model_thinking": "glow",
      "when_tool_started": "nod",
      "when_model_done_responding": "bounce",
      "when_error": "shake",
      "when_stopped": "nod"
    }
  },
  "audio": {
    "when_model_done_responding": {
      "volume": 0.15,
      "notes": [
        { "frequency": 660, "durationMs": 100 },
        { "frequency": 880, "durationMs": 160 }
      ]
    }
  }
}
```

`avatar` and `audio` are optional. Unknown fields and event names are rejected.
JSON exports contain the definition, not credentials, sessions, or feature switches.
Native apps use the OS save dialog; browsers download a `.persona.json` file.

## Events

| Event | When it fires |
| --- | --- |
| `when_user_submits_prompt` | The UI queues a submitted prompt. A rejection can subsequently fire `when_error`. |
| `when_model_thinking` | Live activity enters model processing, including after a tool round. |
| `when_tool_started` | Live activity enters tool execution. |
| `when_model_done_responding` | The active turn finishes successfully and the UI returns to idle. Not each streamed chunk or intermediate tool round. |
| `when_error` | A turn fails or a submitted prompt is rejected. |
| `when_stopped` | An active turn is stopped/cancelled. |

Restoring history, changing sessions, and reconnecting do not replay old events.
Sounds stop when disabled, switched, or the app is unloaded. Once enabled, completion
sounds can play while you are in another tab; browser/device suspension and autoplay
policies still apply. Audio permission is unlocked by a user gesture; enable sounds
and select **Test sound** if needed.
Playback failures appear in the Personas screen and never block the model.

## SVG artwork and animation

Use a transparent SVG with a `viewBox`, ideally `0 0 100 100`. The host displays it
as an isolated image, not inline executable markup. Artwork is never a chat input
hit target. Event animations are supplied by the host: **bounce**, **nod**, **shake**,
**spin**, **glow** (650 ms), or **pace** (1.8 seconds). No animation scripts are imported.

The `pace` animation moves the SVG companion left and right. When mapped to
`when_model_thinking`, it repeats until thinking ends; other events and the
preview play it once. Reduced-motion preferences disable avatar movement.

Allowed elements: `svg`, `g`, `path`, `circle`, `ellipse`, `rect`, `line`, `polyline`,
`polygon`, `text`, `tspan`, `defs`, `linearGradient`, `radialGradient`, `stop`,
`title`, and `desc`. Common geometry, fill/stroke, opacity, transform, gradient,
and text attributes are supported. Gradients may use `url(#local-id)`.
Scripts, style attributes/stylesheets, event handlers, links, images, external
resources, SMIL, `foreignObject`, entities, and XML declarations are rejected.
Use the JSON event map to animate the image instead.

For continuous movement of parts within the image, add an optional
`avatar.animations` array. Skippy uses this for floating, face twitches, and
swaying epaulettes. Each target must be an existing SVG element ID, used once:

```json
"animations": [
  {"target": "floating-unit", "motion": "float", "origin": [450, 500]},
  {"target": "drawn-face", "motion": "twitch", "origin": [450, 400]},
  {"target": "epaulette-left", "motion": "sway-left", "origin": [300, 480]},
  {"target": "epaulette-right", "motion": "sway-right", "origin": [600, 480]}
]
```

The host supplies these four fixed transform animations; imported CSS is still
rejected. Origins use SVG viewBox coordinates (each from -10,000 to 10,000).
At most eight parts can animate. These loops work in the companion and preview,
remain inside an isolated image, and stop under the system's reduced-motion
preference. JSON exports retain both the artwork and animation mappings.

## Sounds

A sound has `volume` (0–1) and exactly one of:

- `notes`: 1–4 sine tones, each 40–2000 Hz and 30–500 ms. The host applies a soft
  envelope and conservative gain.
- `src`: an embedded `data:audio/wav;base64,...`, `audio/mpeg`, `audio/ogg`, or
  `audio/webm` URL. Supply audio you can use. Codec support depends on the device.
  Clips stop after four seconds; only one persona sound plays at a time.

Remote audio URLs and JavaScript are not supported. Importing a file does not
activate sound if the Event sounds switch is off. Enabling sound does not enable
Network access or modify runtime permissions.

## Limits

- JSON file: **2 MiB**; library: **20 custom definitions / 4 MiB**.
- ID: lowercase letters, digits, hyphens; starts with a letter; maximum 64 characters.
- Name: 80 characters; description: 500; narration: 2,000.
- Quips: 1–200, each at most 240 characters.
- SVG source: 32 KiB / 512 elements.
- Audio data URL: 512 KiB per event.
- Generated system section: 8 KiB UTF-8 in the command protocol.

Storage/quota errors keep the current in-memory choice and display an error.
Export your custom definition if local persistence is unavailable.
