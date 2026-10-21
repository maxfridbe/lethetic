# Control a native session from the browser

[All modes](../../README.md#how-to-guides) · [Remote-control reference](../web-remote-control.md)

Remote control uses the same native engine, model connection, tools, and session
as the terminal. Code executes on the CLI machine under its selected isolation
policy. The browser is the controller.

## Start the controller

1. Complete [native setup](native.md), then run:

   ```bash
   lethetic --rc
   ```

2. Choose the bind address, port, authentication, file sharing, and interface when
   prompted. Lethetic prints equivalent flags for reuse.
3. Open the printed controller URL. For a generated self-signed certificate,
   compare its fingerprint with the one printed by Lethetic before accepting it.
4. Acknowledge the startup information in the terminal. Submit prompts, approve
   tools, and manage the current session from the web UI.

Keep the controller-token URL private: it grants control of the native session.
The browser receives a redacted state projection, while provider credentials and
tool execution stay with the native process.

## Common launch options

```bash
# Local controller with optional terminal UI.
lethetic --rc 127.0.0.1

# Enable read-only browsing of eligible launch-directory files.
lethetic --rc 127.0.0.1 --rc-files

# Browser controller only; the engine remains a native foreground process.
lethetic --rc 127.0.0.1 --rc-only

# Use the same controller with isolated C# execution.
lethetic --rc 127.0.0.1 --rc-files --dotnet-only
```

The C# example requires the [.NET container image](dotnet.md#native-net).
`-rc` and `--rc` are equivalent. File sharing is optional and read-only; it does not
change model filesystem grants. Disconnecting the browser does not stop Lethetic.

You can also start remote control from an existing terminal session with
**Ctrl+P → Remote Control: start**. For LAN/VPN binding, custom certificates,
authentication options, and service lifecycle, use the
[remote-control reference](../web-remote-control.md).
