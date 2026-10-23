# Embed the harness

[All modes](../../README.md#how-to-guides) · [Embedding API reference](../../integration.md#embed-the-complete-ui)

Use the browser release to put Lethetic's shared web UI in your application, or
use its headless SDK to supply your own interface.

## Embed the web UI

1. Download `lethetic-browser.zip` from a
   [release](https://github.com/maxfridbe/lethetic/releases/latest).
2. Extract it under a directory such as `/vendor/lethetic/` on your web server.
   Keep the extracted trees and stylesheets together. Serve `.wasm` as
   `application/wasm` and `.js` as JavaScript.
3. Add a container and initialize the UI:

   ```html
   <link rel="stylesheet" href="/vendor/lethetic/styles.css">
   <div id="assistant" style="height: 80vh"></div>
   <script type="module">
     import { mountLethetic, openBrowserStorage } from '/vendor/lethetic/src/embed.js';
     const storage = await openBrowserStorage(localStorage);
     const view = mountLethetic(document.getElementById('assistant'), {
       connections: [{
         name: 'My service', provider: 'openai', model: 'my-model',
         endpoint: 'https://models.example.com/v1/chat/completions'
       }],
       storage
     });
     // Await storage.flush(), then view.destroy() and storage.close() when removing it.
   </script>
   ```

Replace the endpoint and model with your service. Browser CORS rules apply.
Let users supply their own credentials or use an authenticated server proxy;
do not publish a shared secret in client-side HTML. Passing storage enables local,
unencrypted Turso/SQLite persistence; the [API reference](../../integration.md#embed-the-complete-ui)
describes storage adapters, credentials, runtime options, and session routing.

## Use your own interface

Use the [headless embedding SDK](../../integration.md#use-the-headless-embedding-sdk)
for programmatic conversations, streaming events, and custom tools. This SDK runs
the browser host; it does not start a native CLI process or grant host tool access.

For a locally built distribution, follow [build and test](../../integration.md#build-and-test).
For publishing, see [Pages and releases](../../integration.md#github-pages-and-releases).
