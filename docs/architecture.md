# Execution, isolation, and browser architecture

These diagrams describe the current implementation. GitHub renders the Mermaid
blocks directly. Boxes group execution locations; arrows show commands, data, or
allowed access. A grouped box alone does not imply a security boundary.

## Isolation modes

The **tool profile** determines what the model can call. The **execution backend**
determines where that code runs and what it can access. Choosing a browser UI does
not change the native process's tool isolation policy.

```mermaid
flowchart TD
    Profile{"Native tool profile"}
    Profile --> General["General tools: files, shell, and other native capabilities"]
    General --> HostOS["Host OS: authority depends on the tool and approval policy"]
    Profile --> Python["Python-only: one model tool, python"]
    Python --> Backend{"Configured Python backend"}
    Backend --> Host["Host Python: no sandbox"]
    Backend --> Bwrap["Bubblewrap: Linux namespaces and selected filesystem mounts"]
    Backend --> Transient["Transient rootless Podman: read-only root and explicit mounts"]
    Backend --> Retained["Retained rootless Podman: chat-owned package layer and broker"]
    Host --> HostOS
    Profile --> Dotnet[".NET-only: one model tool, dotnet"]
    Dotnet --> Fixed["Fixed isolated Podman policy: network off; launch workspace writable"]
    Fixed --> Supervisor["Python IPC supervisor inside container"]
    Supervisor --> CSharp["Persistent C# runspace and .NET SDK inside container"]
```

| Backend | Filesystem access | Tool networking | Lifetime |
| --- | --- | --- | --- |
| Host Python | Host user's filesystem permissions; cwd is not confinement | Host network | Worker process |
| Bubblewrap Python | Selected runtime files, workspace, and configured grants in Linux namespaces | None or Full; Nonlocal is unsupported | Worker process |
| Transient Podman Python | Read-only container root, temporary directories, workspace and explicit grants | None or Full | Worker/container |
| Retained Podman Python | Chat-owned writable package layer, workspace, and narrowly scoped broker resources | Nonlocal broker; direct container network disabled | Retained chat runtime with 14-day TTL |
| Native .NET | Read-only container root, temporary directories, writable launch workspace; `.lethetic` masked | Always None | Persistent runspace in transient Podman |
| Browser Pyodide / DotnetDide | Browser virtual filesystem synchronized with session files | Browser HTTP subject to permissions and CORS; optional HTTP relay | Worker; files persist separately |

Native .NET uses the same isolated Podman infrastructure as Python. Its SDK and
internal Python supervisor are in the image; it does not fall back to a host SDK
or DotnetDide. Browser .NET uses DotnetDide instead. Neither a Python-only nor a
.NET-only tool surface, by itself, describes a filesystem security boundary.

### Native container network policies

```mermaid
flowchart LR
    subgraph Native["Native Lethetic process"]
        Engine["Conversation engine"]
        Broker["Nonlocal broker: identity, DNS, route, and destination checks"]
    end
    Engine -->|"provider HTTP requests"| Provider["Configured model endpoint"]
    Engine -->|"execute tool"| Choice{"Container network policy"}
    Choice --> None["None: isolated network namespace"]
    None --> Block["No external network route"]
    Choice --> Nonlocal["Nonlocal: retained container; direct network disabled"]
    Nonlocal -->|"authenticated Unix-domain protocol"| Broker
    Broker -->|"permitted public HTTP port 80 / HTTPS port 443"| Public["Public services"]
    Choice --> Full["Full: host networking"]
    Full --> Reachable["Host localhost, LAN, VPN, and Internet via host routes"]
```

The model connection runs outside the tool container. **Disabling tool network
access does not disable model requests.** Nonlocal blocks local/private and other
non-global destinations; it is not a domain allowlist. Full allows host network
reachability but does not grant host filesystem access by itself.

These invocation-locked modes use the launch directory at the same absolute path,
read/write, mask its `.lethetic` control directory, and disallow extra path grants:

| Invocation | Backend and network |
| --- | --- |
| `--python-fully-isolated` / `--python-only isolated` | Transient Podman, None |
| `--python-isolated-with-nonlocal-network` / `--python-only nonlocal` | Retained Podman, Nonlocal, explicit session package helper |
| `--python-isolated-permissive` / `--python-only permissive` | Transient Podman, Full |
| `--dotnet-only` | Transient Podman, fixed None policy |

Ordinary configured Python policies can have different workspace bindings and
grants; retained Nonlocal configuration uses a managed workspace unless selected
through a literal CLI mode. Backends fail closed rather than silently falling
back to Host. Containers and namespaces share the host kernel; they are not VMs.
See [Python isolation](python-isolation.md) and [native .NET](dotnet-native.md) for
the full policy and lifecycle rules.

## Remote control: `-rc` / `--rc`

Remote control serves the shared web UI from the native application. The native
machine owns the conversation, credentials, provider requests, tools, and durable
session. The browser sends commands and displays a redacted projection of state.

```mermaid
flowchart LR
    subgraph Browser["Controller browser"]
        UI["Shared web UI: chat, palette, approvals, and Files"]
        Transport["WebSocket transport"]
        UI <-->|"typed commands and snapshots"| Transport
    end
    subgraph Machine["Machine running lethetic --rc"]
        Server["HTTPS / WSS listener: authentication and exact Host / Origin checks"]
        Actor["Single native application / provider / WFE actor"]
        TUI["Optional terminal UI"]
        Tools["Native tool dispatcher and selected isolation policy"]
        State["Native session persistence"]
        Files["Optional read-only launch-directory file API"]
        Server <-->|"commands and redacted state"| Actor
        TUI <--> Actor
        Actor <--> Tools
        Actor <--> State
        Server --> Files
    end
    Transport <-->|"authenticated WSS"| Server
    UI -->|"authenticated HTTPS file requests"| Server
    Actor <-->|"provider HTTP and streaming"| Provider["Model endpoint"]
    Tools --> Runtime["Host tools or isolated Python / .NET runtime"]
```

The terminal and browser control the same actor. `--rc-only` (alias `--service`)
removes the interactive TUI surface; the engine still runs natively. Browser
disconnects do not terminate the engine. `--rc-files` enables the separate,
read-only file API rooted at the pinned launch directory, subject to disclosure
rules; it does not grant the model new filesystem permissions.

### Default controller authentication

```mermaid
sequenceDiagram
    participant U as User
    participant B as Browser
    participant S as Native HTTPS server
    participant A as Native actor
    S-->>U: Print controller URL with token fragment and TLS fingerprint
    U->>B: Open controller URL and verify certificate
    B->>S: Load UI over HTTPS (fragment is not transmitted)
    Note over B: Bootstrap removes fragment from address bar
    B->>S: Exchange controller token over HTTPS
    S-->>B: Secure HttpOnly SameSite cookie and in-memory WebSocket proof
    B->>S: Open WSS using cookie and proof
    S->>S: Validate exact Host, Origin, cookie, and proof
    B->>S: Typed command: submit prompt
    S->>A: Dispatch command to the current session
    A-->>S: State changes and command result
    S-->>B: Redacted snapshots and response
```

Explicit tokenless mode (`--rc-open`) allows every reachable peer to obtain control;
HTTPS and process-binding checks remain, but do not authenticate the person.
See [remote control](web-remote-control.md) for TLS, file access, and service setup.

## Browser-only mode

GitHub Pages or another static server delivers assets. The browser owns the
conversation and sends provider requests directly. The shared UI uses a local
transport instead of the native WebSocket transport.

```mermaid
flowchart TD
    Static["GitHub Pages / static host"] -->|"HTML, JS, WASM, and runtime assets"| UI
    subgraph Browser["User's browser"]
        UI["Shared SpaApplication: chat, palette, Files, and debugger"]
        Local["LocalTransport: typed command / snapshot adapter"]
        SDK["Browser SDK: provider and tool rounds, cancellation, persistence adapter"]
        Rust["lethetic-wasm bindings to portable CLI Rust code: parsers, context, compaction, retries, loop detection"]
        Workers["Selected code worker: Pyodide or DotnetDide"]
        VFS["Worker virtual filesystem"]
        Workspace["Session workspace and Files pane"]
        Sessions["Turso in shared Rust/WASM: one SQLite image per session in IndexedDB"]
        Preferences["localStorage: connection metadata and preferences"]
        Keys["sessionStorage: tab-scoped API keys"]
        UI <--> Local
        Local <--> SDK
        SDK <--> Rust
        SDK <-->|"tool calls and results"| Workers
        Workers <--> VFS
        VFS <-->|"import / export session files"| Workspace
        Local <--> Workspace
        Local <--> Sessions
        Preferences --> Local
        Keys --> Local
    end
    SDK <-->|"fetch and streaming; endpoint must allow CORS"| Provider["Model endpoint"]
    Workers -->|"Network access enabled"| HTTP["Browser HTTP: CORS or configured relay"]
    HTTP --> Services["External services"]
```

The hosted application stores chats and files unencrypted locally using the
[shared embedded Turso schema](storage.md). Each native session stores content in
`.lethetic/sessions/<session>/lethetic.db`; `python.ipynb` remains a Jupyter notebook file. API keys are
excluded from saved chats and connection metadata; tab-scoped storage retains them
across refresh. Embedding hosts can supply their own storage options. Restoring a
session restores files and history, not worker variables or running programs.
Python's session root is `/home/pyodide`; .NET's is `/home/dotnetdide`.

Portable Rust behavior is reused, but full native orchestration parity is not yet
complete: the browser SDK still drives its own model/tool rounds. Browser-only
mode has no native shells, Podman, LSP processes, or unrestricted OS sockets.
See [shared-core status](browser-parity.md) for what is shared and what remains.

### Interactive sites and permissions

```mermaid
flowchart LR
    Frame["Sandboxed opaque Sites iframe: HTML, JS, and relative HTTP requests"]
    Bridge["Virtual-site page bridge: MessageChannel"]
    Permission{"Interactive sites enabled?"}
    Handler["Worker handler: DotnetDide.Wire / ASP.NET or lethetic_wire / WSGI"]
    VFS["Session virtual files: static assets and generated output"]
    Network{"Network access enabled?"}
    Fetch["Browser HTTP / configured relay"]
    Frame <-->|"virtual HTTP messages and responses"| Bridge
    Bridge --> Permission
    Permission -->|"yes"| Handler
    Permission -->|"no"| Denied["Hosting unavailable"]
    Handler <--> VFS
    Handler -->|"outbound HTTP"| Network
    Network -->|"yes"| Fetch
    Network -->|"no"| Blocked["Outbound request blocked"]
```

Virtual ports route browser requests to worker handlers; they do not open TCP
listeners on the user's machine. The current Sites preview uses an opaque iframe
and a message bridge, not a service worker. .NET route responses support streaming;
Python uses the buffered response adapter. Classic scripts and relative site
requests work; direct external requests from iframe JavaScript, ES modules, and
WebSockets are unavailable. External HTTP goes through the Python or C# handler.
Interactive sites and outbound Network access
are separate, default-off permissions. A local site can run without external
network access. NuGet package access is a third, separate .NET permission, also
off by default; compatible packages use browser downloads and Cache Storage.
Browser network permission does not bypass CORS or enable raw SOCKS sockets;
a configured native HTTP relay can provide upstream proxy routing.

See the [browser integration guide](../integration.md),
[Python sites](../integration.md#interactive-python-sites), and
[.NET sites](../integration.md#run-saved-programs-and-interactive-sites) for examples.

## Implementation map

| Diagram area | Implementation |
| --- | --- |
| Native Python policy and backends | [`src/config.rs`](../src/config.rs), [`src/python/backend.rs`](../src/python/backend.rs), [`src/python/retained_podman.rs`](../src/python/retained_podman.rs) |
| Native .NET isolation and supervisor | [`src/dotnet/mod.rs`](../src/dotnet/mod.rs), [`runtime/dotnet`](../runtime/dotnet) |
| Remote listener, authentication, actor, and file API | [`src/wfe`](../src/wfe), [`web/src/transport.ts`](../web/src/transport.ts) |
| Shared web UI | [`web/src/app.tsx`](../web/src/app.tsx) |
| Browser transport, engine adapter, and persistence | [`browser/src/local-transport.ts`](../browser/src/local-transport.ts), [`browser/src/sdk.ts`](../browser/src/sdk.ts), [`browser/src/session-storage.ts`](../browser/src/session-storage.ts) |
| Portable Rust exports | [`lethetic-wasm/src/lib.rs`](../lethetic-wasm/src/lib.rs), [`src/engine`](../src/engine) |
| Browser workers and virtual sites | [`browser/src/python.ts`](../browser/src/python.ts), [`browser/src/dotnet.ts`](../browser/src/dotnet.ts), [`browser/src/sites-dialog.ts`](../browser/src/sites-dialog.ts) |
