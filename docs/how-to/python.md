# Work in Python

[All modes](../../README.md#how-to-guides) · [Python isolation reference](../python-isolation.md)

Python mode gives the model one execution tool, `python`, with a persistent
runspace. Choose browser Pyodide for session-local files, or native Python for
project work with an explicit isolation policy.

## Browser Pyodide

1. [Connect in the browser](browser.md), then select **Python · Pyodide** in
   **Connections → Runtime & security**.
2. Upload inputs in **Files** and ask the model to write and run a Python program.
   For example: “Summarize this CSV and save a chart and a reusable `analysis.py`.”
3. Preview or download the results from **Files**. Session files are available to
   Python under `/home/pyodide` and survive refresh; Python variables do not.

Enable **Network access** if the program needs outbound HTTP. Browser CORS still
applies, or you can configure the supported HTTP relay. For a local interactive
site, enable **Interactive sites**, run a `.py` file using `lethetic_wire.listen`,
and open **Sites**. Hosting and outbound networking are separate permissions.

See the [Pyodide examples](../../integration.md#python-mode-with-pyodide) and
[Python site example](../../integration.md#interactive-python-sites) for async
handlers, static assets, and Flask/WSGI applications.

## Native Python

1. Complete [native setup](native.md) and configure rootless Podman.
2. Ensure the configured Python OCI image exists locally. The default transient
   image is `docker.io/library/python:3.13-slim`; acquire it explicitly if needed.
   Lethetic does not pull an image as a side effect of running a cell.
3. Choose the network policy and start:

   ```bash
   # No tool network access; launch directory mounted read/write.
   lethetic --python-only isolated

   # Host, localhost, LAN, VPN, and Internet reachability.
   lethetic --python-only permissive
   ```

These flags lock the profile for that process. Both use transient rootless Podman,
mask `.lethetic`, and allow no extra host path grants. Model-provider requests
still work when tool networking is disabled.

For public HTTP(S) through a constrained broker and retained session packages,
first [build the trusted runtime image](../python-isolation.md#building-the-trusted-runtime-image),
then use:

```bash
lethetic --python-only nonlocal --new-session \
  --command "Inspect this project and run its tests"

lethetic --python-only nonlocal --session-id YOUR_SESSION_UUID \
  --command "Continue"
```

The retained package layer survives session resume; Python globals restart.
Headless Nonlocal runs require a durable session identity. Managed package changes
use the explicit `lethetic-pkg` helper described in the isolation reference.

Host and Bubblewrap are additional configurable Python backends. Host Python is
not sandboxed; see [backend and policy details](../python-isolation.md#backends)
before selecting it. Native Python executions are recorded in the
[session notebook audit](../python-isolation.md#notebook-audit).
