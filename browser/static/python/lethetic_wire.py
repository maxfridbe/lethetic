"""Virtual HTTP sites for Lethetic's Pyodide worker; no socket or server loop."""
import asyncio
import ast
import base64
from dataclasses import dataclass, field
import inspect
import io
import json
import mimetypes
import os
from pathlib import Path
import re
import sys
import traceback
from urllib.parse import parse_qs, unquote

MAX_BODY = 64 * 1024 * 1024
MAX_REQUEST_BODY = 16 * 1024 * 1024
REQUEST_TIMEOUT = 15
_enabled = False
_workspace = Path("/home/pyodide")
_sites = {}
_token = re.compile(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$")
_private_headers = {"cookie", "authorization", "host", "content-length",
                    "connection", "transfer-encoding"}


@dataclass
class Request:
    method: str
    path: str
    query: dict
    headers: dict
    body: bytes
    query_string: str = ""

    def text(self):
        return self.body.decode("utf-8")

    def json(self):
        return json.loads(self.body)


@dataclass
class Response:
    body: bytes | str = b""
    status: int = 200
    headers: dict = field(default_factory=lambda: {"Content-Type": "text/plain; charset=utf-8"})

    @staticmethod
    def json(value, status=200):
        return Response(json.dumps(value, ensure_ascii=False), status,
                        {"Content-Type": "application/json; charset=utf-8"})


@dataclass
class Site:
    port: int
    app: object
    handler: object
    directory: Path
    web_root: Path | None

    def stop(self):
        return stop(self.port)


def _configure(enabled, workspace="/home/pyodide"):
    global _enabled, _workspace
    if type(enabled) is not bool:
        raise TypeError("Interactive sites permission must be a boolean.")
    root = Path(workspace).absolute()
    if not enabled or root != _workspace:
        _sites.clear()
    _workspace, _enabled = root, enabled


def _local_path(value):
    path = Path(os.path.abspath(value))
    if not path.is_relative_to(_workspace) or path.resolve() != path:
        raise ValueError("Site paths must be inside the session workspace, without symlinks.")
    return path


def listen(port, app=None, *, handler=None, web_root=None):
    """Register a WSGI app, sync/async Request handler, or static directory; return immediately."""
    if not _enabled:
        raise PermissionError("Interactive sites are disabled. Enable them in Connections → Runtime & security.")
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("Port must be an integer from 1 to 65535.")
    if port in _sites:
        raise ValueError(f"Port {port} is already running; stop it before rerunning.")
    if len(_sites) >= 8:
        raise ValueError("At most eight sites may run in this worker.")
    if app is not None and handler is not None:
        raise ValueError("Choose a WSGI app or a Request handler.")
    if (app is not None and not callable(app)) or (handler is not None and not callable(handler)):
        raise TypeError("The app or handler must be callable.")
    if app is None and handler is None and web_root is None:
        raise ValueError("Provide an app, handler, or web_root.")
    directory = _local_path(Path.cwd())
    root = _local_path(web_root) if web_root is not None else None
    if root is not None and not root.is_dir():
        raise ValueError("The web_root directory must exist.")
    site = Site(port, app, handler, directory, root)
    _sites[port] = site
    return site


def stop(port):
    return _sites.pop(port, None) is not None


async def run_file(path):
    """Run a session .py file, including top-level await, from its own directory."""
    file = _local_path(path)
    source = file.read_text(encoding="utf-8")
    if sys.platform == "emscripten":
        from pyodide_js import loadPackagesFromImports
        await loadPackagesFromImports(source)
    directory, search_path = Path.cwd(), sys.path[:]
    try:
        os.chdir(file.parent)
        sys.path.insert(0, str(file.parent))
        namespace = {"__name__": "__main__", "__file__": str(file), "__package__": None}
        result = eval(compile(source, str(file), "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT), namespace)
        if inspect.isawaitable(result):
            await result
    finally:
        os.chdir(directory)
        sys.path[:] = search_path


def _sites_json():
    return json.dumps([{"port": s.port, "webRoot": str(s.web_root.relative_to(_workspace)) if s.web_root else None}
                       for s in _sites.values()])


def _headers(values):
    if not isinstance(values, dict) or len(values) > 64:
        raise ValueError("Too many headers.")
    result, size = {}, 0
    for key, items in values.items():
        if not isinstance(key, str) or not _token.fullmatch(key):
            raise ValueError("Invalid header name.")
        items = [items] if isinstance(items, str) else items
        if not isinstance(items, list):
            raise ValueError("Invalid header values.")
        for value in items:
            if not isinstance(value, str) or any(ord(c) < 32 or ord(c) == 127 for c in value):
                raise ValueError("Invalid header value.")
            size += len(key.encode()) + len(value.encode())
        if size > 16384:
            raise ValueError("Headers exceed 16 KiB.")
        result[key] = items
    return result


def _encode(response, method="GET", error=None):
    if not isinstance(response, Response):
        raise TypeError("A site handler must return lethetic_wire.Response.")
    if type(response.status) is not int or not 200 <= response.status <= 599:
        raise ValueError("Response status must be between 200 and 599.")
    body = response.body.encode("utf-8") if isinstance(response.body, str) else response.body
    if not isinstance(body, bytes):
        raise TypeError("Response body must be bytes or str.")
    if len(body) > MAX_BODY:
        raise ValueError("Response body exceeds 64 MiB.")
    headers = {k: v for k, v in _headers(response.headers).items()
               if k.lower() not in {"set-cookie", "connection", "transfer-encoding", "content-length"}}
    if method == "HEAD" or response.status in {204, 205, 304}:
        body = b""
    return {"status": response.status, "headers": headers,
            "body": base64.b64encode(body).decode("ascii"), "error": error}


def _error(status, message):
    return _encode(Response(message, status), error=message)


def _wsgi(site, request):
    env = {
        "REQUEST_METHOD": request.method, "SCRIPT_NAME": "",
        "PATH_INFO": request.path.encode("utf-8").decode("latin-1"),
        "QUERY_STRING": request.query_string, "SERVER_NAME": "localhost", "SERVER_PORT": str(site.port),
        "SERVER_PROTOCOL": "HTTP/1.1", "CONTENT_LENGTH": str(len(request.body)),
        "wsgi.version": (1, 0), "wsgi.url_scheme": "http", "wsgi.input": io.BytesIO(request.body),
        "wsgi.errors": sys.stderr, "wsgi.multithread": False, "wsgi.multiprocess": False,
        "wsgi.run_once": False,
    }
    for key, values in request.headers.items():
        name = key.upper().replace("-", "_")
        env[name if name == "CONTENT_TYPE" else "HTTP_" + name] = ", ".join(values)
    chunks, status, headers, size = [], None, {}, 0

    def write(value):
        nonlocal size
        if status is None or not isinstance(value, bytes):
            raise TypeError("WSGI must start the response and write bytes.")
        size += len(value)
        if size > MAX_BODY:
            raise ValueError("Response body exceeds 64 MiB.")
        chunks.append(value)

    def start_response(value, pairs, exc_info=None):
        nonlocal status, headers
        if exc_info and chunks:
            raise exc_info[1].with_traceback(exc_info[2])
        if status is not None and not exc_info:
            raise RuntimeError("WSGI response already started.")
        if not isinstance(value, str) or not re.fullmatch(r"[2-5][0-9]{2} [^\r\n]+", value):
            raise ValueError("Invalid WSGI status.")
        status, headers = int(value[:3]), {}
        for key, item in pairs:
            headers.setdefault(key, []).append(item)
        _headers(headers)
        return write

    iterator = site.app(env, start_response)
    try:
        for chunk in iterator:
            write(chunk)
    finally:
        close = getattr(iterator, "close", None)
        if close:
            close()
    if status is None:
        raise RuntimeError("WSGI app did not start a response.")
    return Response(b"".join(chunks), status, headers)


async def _serve(site, request):
    previous = Path.cwd()
    try:
        os.chdir(site.directory)
        if site.app is not None:
            response = _wsgi(site, request)
        elif site.handler is not None:
            response = site.handler(request)
            if inspect.isawaitable(response):
                response = await response
        else:
            response = Response("Not found", 404)
        if isinstance(response, Response) and response.status == 404 and site.web_root and request.method in {"GET", "HEAD"}:
            path = site.web_root / request.path.lstrip("/")
            if path.is_dir():
                path = path / "index.html"
            if path.is_relative_to(site.web_root) and path.resolve() == path and path.is_file():
                if path.stat().st_size > MAX_BODY:
                    return Response("File exceeds 64 MiB.", 413)
                mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
                response = Response(path.read_bytes(), headers={"Content-Type": mime})
        return response
    finally:
        os.chdir(previous)


async def _dispatch_json(payload):
    try:
        return json.dumps(await _dispatch(json.loads(payload)), ensure_ascii=True)
    except Exception:
        return json.dumps(_error(500, traceback.format_exc()))


async def _dispatch(value):
    if not _enabled:
        return _error(403, "Interactive sites are disabled.")
    method, target = value.get("method"), value.get("path")
    if method not in {"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"}:
        return _error(405, "Unsupported method.")
    if not isinstance(target, str) or len(target) > 8192 or not target.startswith("/") or target.startswith("//") or "#" in target:
        return _error(400, "Invalid request path.")
    raw_path, _, query = target.partition("?")
    try:
        path = unquote(raw_path, errors="strict")
        if "\\" in path or any(ord(c) < 32 or ord(c) == 127 for c in path) or any(p in {".", ".."} for p in path.split("/")):
            return _error(400, "Unsafe request path.")
        encoded = value.get("body", "")
        if not isinstance(encoded, str) or len(encoded) > (MAX_REQUEST_BODY + 2) // 3 * 4:
            return _error(413, "Request body exceeds 16 MiB.")
        body = base64.b64decode(encoded, validate=True)
        if len(body) > MAX_REQUEST_BODY:
            return _error(413, "Request body exceeds 16 MiB.")
        headers = _headers(value.get("headers", {}))
        headers = {k.lower(): v for k, v in headers.items() if k.lower() not in _private_headers}
        parsed = parse_qs(query, keep_blank_values=True, max_num_fields=256)
    except (ValueError, UnicodeError) as error:
        return _error(431 if "headers" in str(error).lower() or "16 KiB" in str(error) else 400, str(error))
    port = value.get("port")
    site = _sites.get(port) if type(port) is int else None
    if site is None:
        return _error(503, "Virtual site is not running.")
    request = Request(method, path, parsed, headers, body, query)
    try:
        response = await asyncio.wait_for(_serve(site, request), REQUEST_TIMEOUT)
        return _encode(response, method)
    except TimeoutError:
        return _error(504, "Site request timed out.")
