# Run the native harness

[All modes](../../README.md#how-to-guides) · [Native reference](../native-reference.md)

Use the native application for terminal interaction, scripted runs, and tools that
operate on your local project. Lethetic connects to an existing model service; it
does not install or launch the model server.

## Configure and build

1. Create `config.yml` in your project or `~/.config/lethetic/config.yml` using the
   [configuration example](../native-reference.md#example-configyml). Set your
   endpoint, model, and connection kind. Keep credentials out of version control.
2. From the Lethetic repository, build the application:

   ```bash
   cargo build --release --bin lethetic
   ```

3. Run the resulting `target/release/lethetic` executable from the project you
   want to work on. Add its directory to your PATH to use the commands below.

## Interactive terminal

```bash
lethetic
```

Enter a task and use **Ctrl+P** to select a model or change settings. **F12** opens
the debugger, **F9** the model's todo list, and **F8** background tasks. General mode
offers native tools subject to their approval policies. Select a restricted tool
profile with [Python-only](python.md#native-python) or [.NET-only](dotnet.md#native-net).

Resuming a session restores its model, prompt, mode, theme, and loop-detection
settings. See the [shortcut reference](../native-reference.md#tui-hotkeys).

## Headless commands

```bash
lethetic --command "Inspect this project and summarize its test setup"

# Create a durable session; keep the printed session ID to resume it.
lethetic --new-session --command "Investigate the failing tests"

lethetic --session-id YOUR_SESSION_UUID --command "Continue the investigation"
```

Put `--command` last: it consumes the remaining command-line text. The same tool
profiles are available in headless runs; configure required runtimes first.

To use the web UI with this native engine, follow [remote control](remote-control.md).
For provider setup, tool behavior, context budgets, and retries, see the
[native reference](../native-reference.md).
