# Speech endpoint compatibility

Lethetic's **Personas → Speech playback** supports OpenAI-style **PCM streaming**
or **WAV file** responses, and ElevenLabs-style **PCM streaming**. This is a
speech API call from the UI, independent of model chat and code-runtime permissions.
Use a speech route as the endpoint, not `/openapi.yml` or `/openapi.json`.

## WAV file endpoints

Select **OpenAI-compatible**, set **Audio format → WAV**, and enter the full
speech route, a model and a voice that the server supports. Lethetic sends:

```http
POST /v1/audio/speech
Authorization: Bearer <key>
Content-Type: application/json
Accept: audio/wav, application/octet-stream

{"model":"your-tts-model","voice":"your-voice","input":"Hello there.","response_format":"wav"}
```

Return `200 OK` with `Content-Type: audio/wav` and the **binary RIFF/WAVE file**.
`audio/x-wav`, `audio/wave`, `audio/vnd.wave` and `application/octet-stream` are
also accepted. The file must contain audio that the browser can decode; its header
determines sample rate and channel count. Lethetic downloads the complete file,
decodes it using Web Audio, and then plays it. Stop cancels both download and
playback. This mode does not promise playback before the file has arrived.

Responses containing JSON with a URL, base64 audio, or a job ID are not file
responses and are not followed. Choose `response_format: "wav"`, not `"url"`.
WAV downloads are capped at **64 MiB per request** to bound whole-file memory use.
Long response text is divided into requests of at most 4,000 characters; WAV
recordings play in order, with only one file downloaded/decoded at a time.

For the `nextcuda` API inspected at `http://brainiac-nvidia:8000/openapi.yml`,
`/v1/audio/speech` advertises the needed `response_format: "wav"` request. Configure
a speech model and its supported voice. **In an executable app, use
`http://brainiac-nvidia:8000/v1/audio/speech` directly**: Rust performs the HTTP
request and streams the response through the authenticated native bridge, outside
WebView CORS and mixed-content restrictions. The hostname must resolve from the
device. `localhost` on Android means the phone, not the server.

In browser-only or remote-control web pages, HTTP URLs are accepted by Lethetic,
but browser security and the page's Content Security Policy still apply. The
hosted browser page allows HTTP only to its permitted loopback hosts; an HTTPS
page may also block HTTP as mixed content. Use an HTTPS reverse proxy with a
trusted certificate and CORS for the hosted page. This feature does not relax
the page's connection policy.

## OpenAI-compatible PCM streaming

Implement this route (or configure its full alternative URL in Lethetic):

```http
POST /v1/audio/speech
Authorization: Bearer <key>
Content-Type: application/json
Accept: application/octet-stream

{"model":"your-tts-model","voice":"your-voice","input":"Hello there.","response_format":"pcm"}
```

Respond with `200 OK`, `Content-Type: audio/pcm` or `application/octet-stream`,
and stream **headerless signed 16-bit little-endian PCM, mono, 24,000 Hz**. Send the
first samples as soon as synthesis produces them, then flush more samples as they
become available. Close the response after the final sample. Do not wrap the bytes
in JSON, base64, WAV headers, SSE events, or multipart boundaries. OpenAI documents
this PCM format and streaming behavior in its
[speech guide](https://developers.openai.com/api/docs/guides/text-to-speech).

## ElevenLabs-compatible PCM streaming

Implement this route, with the voice in the path:

```http
POST /v1/text-to-speech/{voice_id}/stream?output_format=pcm_24000
xi-api-key: <key>
Content-Type: application/json
Accept: application/octet-stream

{"model_id":"your-tts-model","text":"Hello there."}
```

Configure the endpoint in Lethetic with the literal `{voice_id}` placeholder;
it is replaced with the URL-encoded Voice ID field. Use the same raw mono PCM
sample format and streaming behavior described above. The request uses
`xi-api-key`, not Bearer authentication. Lethetic currently uses PCM only for
this API style; it does not invent a WAV output-format option. See the
[ElevenLabs streaming reference](https://elevenlabs.io/docs/api-reference/text-to-speech/stream).

## Transport, errors and browser access

Only one speech POST route is required for the selected API style. No WebSocket,
SSE, model-list, or voice-list route is required by the current UI. The user enters
model/voice values manually. Implement both POST routes if clients using either
style should work with your server.

For streaming, use the HTTP server's streaming-response facility. HTTP/1.1 may use
chunked transfer encoding; HTTP/2 uses its own framing. Do not set a guessed
Content-Length, and disable reverse-proxy response buffering for the speech route.
Network chunk boundaries need not align with sample boundaries; the final byte
count must be even. At 24 kHz, 16-bit mono, one second is 48,000 bytes. Sending a
complete recording all at once is playable PCM, but does not improve first-audio
latency. Cancel synthesis when the client disconnects where your engine permits.
Lethetic stops requests after 60 seconds without network progress.

Validate authorization, model and voice before sending audio headers. Return a
non-2xx status on failure (for example 401, 400, 429 or 500), optionally with a JSON
error body. Do not return `200 OK` with JSON errors disguised as audio.

The native bridge is restricted to the bundled main UI, with the per-launch host
token checked for every call. It forwards only speech headers, streams in 64 KiB
chunks with a bounded queue, cancels on Stop/session change, and clears old requests
on UI reload. Native speech is a direct user-initiated connection; the code runtime's
network/Tor setting does not route or enable it. No WebView insecure-content toggle
or browser security bypass is needed.

For browser access, handle `OPTIONS` without requiring an API key and include CORS
headers on both the preflight and actual/error responses. For GitHub Pages:

```http
Access-Control-Allow-Origin: https://maxfridbe.github.io
Access-Control-Allow-Methods: POST, OPTIONS
Access-Control-Allow-Headers: Content-Type, Authorization, xi-api-key
Vary: Origin
```

Allow the actual UI origin if hosting elsewhere. HTTPS endpoints need a certificate
trusted by the client device; CORS does not fix an untrusted certificate or HTTP
mixed content. The server hostname must resolve and be reachable from that device.
Lethetic accepts both HTTP and HTTPS, omits browser cookies, and rejects redirects.
HTTP is unencrypted, including any API key and text sent to that endpoint. Keyless servers work too: leaving the key empty omits authentication.
API keys stay in UI memory and are never exported with the persona.

Android native playback accepts streamed PCM WAV with `0xffffffff` for an unknown
data length, as well as ordinary WAV files with a declared size. End-of-stream
completes an unknown-length recording only at a complete audio frame; the cached
WAV is rewritten with its actual size for downloading. An ordinary WAV that ends
before its declared size remains an error.
