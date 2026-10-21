"""Runs only inside the existing isolated Python worker, never on the CLI host."""
import json
import os
import subprocess
import sys
import tempfile

_process = None
_errors = None


def run(code):
    global _process, _errors
    if _process is None or _process.poll() is not None:
        if _errors is not None:
            _errors.close()
        _errors = tempfile.TemporaryFile(mode="w+t")
        env = dict(os.environ)
        env.update(DOTNET_CLI_HOME="/tmp/lethetic-dotnet-cli",
                   NUGET_PACKAGES="/tmp/lethetic-nuget",
                   DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_NOLOGO="1",
                   DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE="1")
        _process = subprocess.Popen(
            ["/usr/share/dotnet/dotnet", "/opt/lethetic-dotnet/out/Lethetic.Dotnet.dll"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=_errors,
            text=True, encoding="utf-8", env=env,
        )
    _process.stdin.write(json.dumps({"code": code, "cwd": os.getcwd()}) + "\n")
    _process.stdin.flush()
    line = _process.stdout.readline(16 * 1024 * 1024 + 1)
    if not line.endswith("\n") or len(line) > 16 * 1024 * 1024:
        _process.kill()
        _process.wait()
        _errors.seek(0)
        raise RuntimeError("Isolated .NET worker stopped or exceeded its reply limit: " + _errors.read(4096))
    reply = json.loads(line)
    # Cwd remains container-local. The shared runtime checks host-visible paths.
    os.chdir(reply["cwd"])
    print(reply["stdout"], end="")
    print(reply["stderr"], end="", file=sys.stderr)
    if reply.get("value"):
        print(reply["value"])
    for diagnostic in reply["diagnostics"]:
        print(diagnostic, file=sys.stderr)
    if not reply["success"]:
        raise RuntimeError(reply.get("error") or "C# compilation failed (see diagnostics)")
