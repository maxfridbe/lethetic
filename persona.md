# Personas

Open the command palette (**Ctrl+P**, or the top-left logo) and select **Personas**.
The palette also has **Import persona JSON** and **Export persona JSON** shortcuts.
The same screen is available in browser-only, native app, and remote-control modes.

Choose **Lethetic — Sassy** (the default), **Skippy the Magnificent**, **Mr. Meeseeks**,
**HAL9000**, or **Space GHOST**. Character presets use original quips and illustrations.
They do not include book/TV dialogue, actor recordings, or cloned voices.

The screen lets you switch personas, inspect their quips and system instructions,
import/export JSON, and independently enable:

- **Narration in system prompt:** applies the selected persona to the next message.
- **Thinking quips:** replaces the rotating gradient status text.
- **Animated SVG companion:** a transparent image above the prompt bar that reacts to events.
- **Event sounds:** optional tones or embedded audio. Off by default.

**Enable persona features** switches all four off without deleting the selection
or individual preferences. Display changes apply immediately, including during a
turn. Narration changes apply on the next submitted message, not halfway through
a response. Reduced-motion preferences suppress SVG animation and quip rotation.

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
