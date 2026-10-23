# Use the browser workspace

[All modes](../../README.md#how-to-guides) · [Browser integration reference](../../integration.md)

Use this mode to run Lethetic without a native process. The conversation runtime
and code workers run in your browser; your chosen provider runs the model.
GitHub Pages serves assets and does not relay your model requests.

## Connect and start

1. Open [Lethetic](https://maxfridbe.github.io/lethetic/) and open **Connections**.
2. Choose **OpenAI-compatible** or **Anthropic Messages**, then enter the full
   endpoint URL, model ID, and API key. Enter the token alone, without `Bearer`.
3. Use **Fetch models** to filter available models, then **Test connection** and
   save. The active connection is marked; its plug turns green after a successful
   test and red after failure.
4. In **Runtime & security**, choose **Chat** to start without a code worker, or
   follow the [Python](python.md#browser-pyodide) or [.NET](dotnet.md#browser-dotnetdide)
   guide for code execution.
5. Send a prompt. Changing the connection updates the current session without
   replacing its history or files.

The provider must allow the page's origin through CORS and use HTTPS for the hosted
page. See [endpoint requirements](../../integration.md#endpoint-requirements).
For OpenRouter, enable **Thinking · OpenRouter** in the connection editor to request
reasoning; readable thinking appears when the model supplies it.

## Work with files and sessions

- Use **Files → Upload** to add inputs. Code workers share the session workspace;
  generated files appear after execution for preview, copy, and download.
- Keep `tasks.md` open as a Markdown preview to follow the model's progress notes.
- Open **Sessions** to resume a chat or **Export debug (.tar.gz)** to download its
  transcript, diagnostics, and files. **Database (.db)** downloads the selected
  chat and files as a SQLite-compatible database.
- Use [latest session](https://maxfridbe.github.io/lethetic/#/latest) or
  [new session](https://maxfridbe.github.io/lethetic/#/new) links. A session URL refers
  to local data; it does not share that chat with another device.

Chats, files, and settings survive refresh when browser storage is available;
worker variables restart. The shared WASM module embeds Turso; IndexedDB holds
a separate database image per session. See [storage and migration](../storage.md). Chats and files are unencrypted on your device. The
hosted app keeps API keys in tab-scoped sessionStorage, outside saved chats.

## Adjust the run

Open **Ctrl+P** for **Turn limits**, **Loop detection**, **System prompt**, or
**Compact session**. Open **Stats** to pin context, usage, and timing to the bottom
bar. Network access and Interactive sites are separate, default-off code-runtime
permissions; .NET also has a separate NuGet permission.

For native shell, host files, LSP, or Podman tools through the web UI, use
[remote control](remote-control.md). To serve the browser edition locally, follow
the [build instructions](../../integration.md#build-and-test), or serve an extracted
[browser release ZIP](https://github.com/maxfridbe/lethetic/releases/latest) over HTTP.
