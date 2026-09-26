import ast
import collections
import json
import os
import sys
import threading
import time
import traceback
import types
import uuid

if os.name == "nt":
    import ctypes
    import ctypes.wintypes
    import msvcrt

    _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _kernel32.PeekNamedPipe.argtypes = [
        ctypes.wintypes.HANDLE,
        ctypes.c_void_p,
        ctypes.wintypes.DWORD,
        ctypes.POINTER(ctypes.wintypes.DWORD),
        ctypes.POINTER(ctypes.wintypes.DWORD),
        ctypes.POINTER(ctypes.wintypes.DWORD),
    ]
    _kernel32.PeekNamedPipe.restype = ctypes.wintypes.BOOL

PROTOCOL_NAME = "LETHETIC_PYTHON"
PROTOCOL_VERSION = 3
WORKER_ABI = "lethetic-python-worker-v4"
OUTPUT_RECOVERY_CAPABILITY = "lethetic-output-v2"
MAX_FRAME_BYTES = 4 * 1024 * 1024
MAX_STREAM_BYTES = 1024 * 1024
MAX_REPR_BYTES = 256 * 1024
MAX_TRACEBACK_BYTES = 256 * 1024
MAX_RESULT_SECTION_BYTES = 64 * 1024
MAX_OUTPUT_ARTIFACTS = 8
MAX_OUTPUT_RING_BYTES = 8 * 1024 * 1024
MAX_OUTPUT_READ_BYTES = 64 * 1024
MAX_HOST_RESPONSE_BYTES = 2 * 1024 * 1024
MAX_HOST_CALLS_PER_CELL = 64
_OUTPUT_SECTIONS = ("stdout", "stderr", "repr", "traceback")

# Keep the framing channel on private, close-on-exec descriptors. In
# particular, process fd 0 is redirected to /dev/null so native reads and user
# subprocesses see EOF instead of inheriting and consuming the control stream.
_control_fd = os.dup(sys.stdin.fileno())
os.set_inheritable(_control_fd, False)
_control_input = os.fdopen(_control_fd, "rb", buffering=0)
_protocol_fd = os.dup(sys.stdout.fileno())
os.set_inheritable(_protocol_fd, False)
_protocol = os.fdopen(_protocol_fd, "wb", buffering=0)
_devnull_fd = os.open(os.devnull, os.O_RDWR)
os.set_inheritable(_devnull_fd, False)
os.dup2(_devnull_fd, 0)
_globals = {"__name__": "__main__", "__builtins__": __builtins__}
_cell_number = 0
_active_request_id = None
_host_sub_id = 0
_host_call_lock = threading.Lock()
_output_ring = collections.OrderedDict()
_output_ring_bytes = 0
_output_ring_lock = threading.RLock()


class _BlockedInput:
    @property
    def buffer(self):
        return self

    def read(self, *_args, **_kwargs):
        raise RuntimeError("interactive stdin is unavailable in the Lethetic Python runspace")

    readline = read

    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


class _Sink:
    def write(self, value):
        return len(value)

    def flush(self):
        return None

    def isatty(self):
        return False


class _BinarySink:
    def write(self, value):
        return len(value)

    def flush(self):
        return None

    def fileno(self):
        return _devnull_fd

    def isatty(self):
        return False


class _SwitchableBinaryStream:
    def __init__(self, sink):
        self._sink = sink
        self._target = sink
        self._lock = threading.RLock()

    def set_target(self, target):
        with self._lock:
            self._target = target if target is not None else self._sink

    def write(self, value):
        with self._lock:
            return self._target.write(value)

    def flush(self):
        with self._lock:
            return self._target.flush()

    def fileno(self):
        with self._lock:
            target = self._target
            return target.fileno() if hasattr(target, "fileno") else _devnull_fd

    def isatty(self):
        with self._lock:
            target = self._target
            return bool(target.isatty()) if hasattr(target, "isatty") else False

    def writable(self):
        return True

    @property
    def closed(self):
        return False


class _SwitchableTextStream:
    def __init__(self, sink, binary_sink):
        self._sink = sink
        self._target = sink
        self._lock = threading.RLock()
        self.buffer = _SwitchableBinaryStream(binary_sink)

    def set_target(self, target):
        with self._lock:
            self._target = target if target is not None else self._sink
            binary = getattr(target, "buffer", None) if target is not None else None
            self.buffer.set_target(binary)

    def write(self, value):
        with self._lock:
            return self._target.write(value)

    def flush(self):
        with self._lock:
            return self._target.flush()

    def fileno(self):
        with self._lock:
            target = self._target
            return target.fileno() if hasattr(target, "fileno") else _devnull_fd

    def isatty(self):
        with self._lock:
            target = self._target
            return bool(target.isatty()) if hasattr(target, "isatty") else False

    def writable(self):
        return True

    @property
    def encoding(self):
        return "utf-8"

    @property
    def errors(self):
        return "backslashreplace"

    @property
    def closed(self):
        return False

    @property
    def name(self):
        return "<lethetic-output>"


def _windows_pipe_bytes_available(fd):
    if os.name != "nt":
        raise RuntimeError("Windows pipe polling was requested on another platform")
    available = ctypes.wintypes.DWORD()
    handle = msvcrt.get_osfhandle(fd)
    success = _kernel32.PeekNamedPipe(
        ctypes.wintypes.HANDLE(handle),
        None,
        0,
        None,
        ctypes.byref(available),
        None,
    )
    if success:
        return available.value
    error = ctypes.get_last_error()
    if error in (109, 232):  # ERROR_BROKEN_PIPE / ERROR_NO_DATA
        return None
    raise ctypes.WinError(error)


class _BoundedPipeCapture:
    """Continuously drain one fd and keep bounded head and tail bytes."""

    def __init__(self, limit):
        self.limit = limit
        self.head_limit = limit // 2
        self.tail_limit = limit - self.head_limit
        self.total = 0
        self.head = bytearray()
        self.tail = collections.deque()
        self.tail_bytes = 0
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.read_fd, self.write_fd = os.pipe()
        os.set_inheritable(self.read_fd, False)
        os.set_inheritable(self.write_fd, False)
        try:
            os.set_blocking(self.read_fd, False)
            self.nonblocking = True
        except (AttributeError, OSError):
            # Older Windows Python versions cannot make anonymous pipe handles
            # nonblocking. PeekNamedPipe polling keeps that fallback stoppable.
            self.nonblocking = False
        self.thread = threading.Thread(target=self._drain, daemon=True)
        self.thread.start()

    def redirect_to(self, target_fd):
        os.dup2(self.write_fd, target_fd)
        os.close(self.write_fd)
        self.write_fd = None

    def _append_tail(self, chunk):
        if not chunk or self.tail_limit == 0:
            return
        chunk = bytes(chunk)
        self.tail.append(chunk)
        self.tail_bytes += len(chunk)
        while self.tail_bytes > self.tail_limit:
            excess = self.tail_bytes - self.tail_limit
            first = self.tail[0]
            if len(first) <= excess:
                self.tail.popleft()
                self.tail_bytes -= len(first)
            else:
                self.tail[0] = first[excess:]
                self.tail_bytes -= excess

    def _record(self, chunk):
        with self.lock:
            self.total += len(chunk)
            head_remaining = self.head_limit - len(self.head)
            if head_remaining > 0:
                take = min(head_remaining, len(chunk))
                self.head.extend(chunk[:take])
                chunk = chunk[take:]
            self._append_tail(chunk)

    def _drain(self):
        stop_deadline = None
        try:
            while True:
                stopping = self.stop.is_set()
                if stopping:
                    if stop_deadline is None:
                        stop_deadline = time.monotonic() + 0.25
                    elif time.monotonic() >= stop_deadline:
                        # A deliberately detached child may retain fd 1/2. Do
                        # not let it hold the cell response open indefinitely.
                        break
                try:
                    if not self.nonblocking and os.name == "nt":
                        available = _windows_pipe_bytes_available(self.read_fd)
                        if available is None:
                            break
                        if available == 0:
                            if stopping:
                                break
                            self.stop.wait(0.01)
                            continue
                        chunk = os.read(self.read_fd, min(64 * 1024, available))
                    else:
                        chunk = os.read(self.read_fd, 64 * 1024)
                except InterruptedError:
                    continue
                except BlockingIOError:
                    if stopping:
                        break
                    self.stop.wait(0.01)
                    continue
                except OSError:
                    break
                if not chunk:
                    break
                self._record(chunk)
        finally:
            try:
                os.close(self.read_fd)
            except OSError:
                pass

    def finish(self):
        if self.write_fd is not None:
            try:
                os.close(self.write_fd)
            except OSError:
                pass
            self.write_fd = None
        self.stop.set()
        self.thread.join(timeout=1.0)
        with self.lock:
            head = bytes(self.head)
            tail = b"".join(self.tail)
            total = self.total
        return _bounded_pipe_text(head, tail, total, self.limit)


_blocked_input = _BlockedInput()
_sink = _Sink()
_binary_sink = _BinarySink()
_stdout_stream = _SwitchableTextStream(_sink, _binary_sink)
_stderr_stream = _SwitchableTextStream(_sink, _binary_sink)


def _encoded_excerpt(value, limit, marker, tail_weight=1):
    encoded = value.encode("utf-8", errors="replace")
    original = len(encoded)
    if original <= limit:
        return value, False, original
    marker_bytes = marker.encode("utf-8", errors="replace")
    if len(marker_bytes) >= limit:
        return marker_bytes[:limit].decode("utf-8", errors="ignore"), True, original
    keep = limit - len(marker_bytes)
    head_keep = keep // (tail_weight + 1)
    tail_keep = keep - head_keep
    head = encoded[:head_keep].decode("utf-8", errors="ignore")
    tail = encoded[-tail_keep:].decode("utf-8", errors="ignore") if tail_keep else ""
    excerpt = head + marker + tail
    # Ignoring split code points makes this no larger than the byte budget.
    if len(excerpt.encode("utf-8")) > limit:
        raise RuntimeError("internal Python output excerpt exceeded its byte limit")
    return excerpt, True, original


def _bounded_pipe_text(head, tail, original, limit):
    if original <= limit:
        raw = head + tail
        truncated = False
    else:
        marker = f"\n... [truncated by Lethetic: {original} bytes total] ...\n".encode()
        keep = max(0, limit - len(marker))
        head_keep = keep // 2
        tail_keep = keep - head_keep
        raw = head[:head_keep] + marker + (tail[-tail_keep:] if tail_keep else b"")
        truncated = True
    value = raw.decode("utf-8", errors="replace")
    value, normalized, _encoded_bytes = _encoded_excerpt(
        value,
        limit,
        f"\n... [truncated by Lethetic: {original} source bytes total] ...\n",
    )
    return value, truncated or normalized, original


def _bounded_string(value, limit):
    original = len(value.encode("utf-8", errors="replace"))
    bounded, truncated, _ = _encoded_excerpt(
        value,
        limit,
        f"\n... [truncated by Lethetic: {original} bytes total] ...\n",
        tail_weight=2,
    )
    return bounded, truncated, original


def _result_excerpt(value, cell, section):
    captured = len(value.encode("utf-8", errors="replace"))
    marker = (
        f"\n... [truncated by Lethetic for display: Python cell {cell} {section}; "
        "retrieve the retained output with lethetic_output.read] ...\n"
    )
    excerpt, _shortened, _ = _encoded_excerpt(
        value,
        MAX_RESULT_SECTION_BYTES,
        marker,
        tail_weight=2 if section in ("stderr", "traceback") else 1,
    )
    return excerpt, captured, len(excerpt.encode("utf-8"))


def _send(payload):
    body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(body) > MAX_FRAME_BYTES:
        raise RuntimeError(
            f"encoded Python worker frame exceeded {MAX_FRAME_BYTES} bytes"
        )
    header = f"{PROTOCOL_NAME} {PROTOCOL_VERSION} {len(body)}\n".encode("ascii")
    _protocol.write(header)
    _protocol.write(body)
    _protocol.flush()


class _TodoError(RuntimeError):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


class _TodoRevisionConflict(_TodoError):
    pass


def _read_host_response(request_id, sub_id):
    raw = _control_input.readline(MAX_HOST_RESPONSE_BYTES + 1)
    if not raw:
        raise RuntimeError("Lethetic host closed the todo capability channel")
    if len(raw) > MAX_HOST_RESPONSE_BYTES:
        raise RuntimeError("Lethetic todo host response exceeded its size limit")
    try:
        response = json.loads(raw)
    except BaseException as error:
        raise RuntimeError("Lethetic todo host response was not valid JSON") from error
    if not isinstance(response, dict):
        raise RuntimeError("Lethetic todo host response was not an object")
    if response.get("id") != request_id or response.get("sub_id") != sub_id:
        raise RuntimeError("Lethetic todo host response identity mismatch")
    if response.get("op") != "host_response" or type(response.get("ok")) is not bool:
        raise RuntimeError("Lethetic todo host response envelope was invalid")
    if response["ok"]:
        if set(response) != {"id", "op", "sub_id", "ok", "result"}:
            raise RuntimeError("Lethetic todo success response contained unexpected fields")
        result = response["result"]
        if (
            not isinstance(result, dict)
            or set(result) != {"revision", "todos"}
            or type(result["revision"]) is not int
            or result["revision"] < 0
            or not isinstance(result["todos"], list)
        ):
            raise RuntimeError("Lethetic todo success response payload was invalid")
        return result
    if set(response) != {"id", "op", "sub_id", "ok", "error"}:
        raise RuntimeError("Lethetic todo error response contained unexpected fields")
    error = response["error"]
    if (
        not isinstance(error, dict)
        or set(error) != {"code", "message"}
        or not isinstance(error["code"], str)
        or not isinstance(error["message"], str)
    ):
        raise RuntimeError("Lethetic todo error response payload was invalid")
    exception_type = (
        _TodoRevisionConflict
        if error["code"] == "revision_conflict"
        else _TodoError
    )
    raise exception_type(error["code"], error["message"])


def _todo_host_call(operation, **payload):
    global _host_sub_id
    with _host_call_lock:
        request_id = _active_request_id
        if request_id is None:
            raise RuntimeError("lethetic_todo is available only while a Python cell is executing")
        _host_sub_id += 1
        if _host_sub_id > MAX_HOST_CALLS_PER_CELL:
            raise RuntimeError(
                f"lethetic_todo permits at most {MAX_HOST_CALLS_PER_CELL} calls per cell"
            )
        sub_id = _host_sub_id
        request = {
            "type": "host_call",
            "id": request_id,
            "sub_id": sub_id,
            "operation": operation,
        }
        request.update(payload)
        _send(request)
        return _read_host_response(request_id, sub_id)


def _todo_get():
    return _todo_host_call("todo.get")


def _todo_set(todos, *, expected_revision):
    if not isinstance(todos, list):
        raise TypeError("todos must be a list")
    if type(expected_revision) is not int or expected_revision < 0:
        raise TypeError("expected_revision must be a non-negative integer")
    return _todo_host_call(
        "todo.set",
        todos=todos,
        expected_revision=expected_revision,
    )


_lethetic_todo = types.ModuleType("lethetic_todo")
_lethetic_todo.__doc__ = "Host-backed, revisioned Lethetic todo list capability."
_lethetic_todo.__all__ = ["get", "set", "TodoError", "RevisionConflict"]
_lethetic_todo.get = _todo_get
_lethetic_todo.set = _todo_set
_lethetic_todo.TodoError = _TodoError
_lethetic_todo.RevisionConflict = _TodoRevisionConflict
sys.modules["lethetic_todo"] = _lethetic_todo


def _validate_artifact_id(artifact_id):
    if not isinstance(artifact_id, str):
        raise TypeError("artifact_id must be the UUID string returned with Python output")
    try:
        parsed = uuid.UUID(artifact_id)
    except (ValueError, AttributeError) as error:
        raise ValueError("artifact_id must be a canonical UUIDv4 string") from error
    if str(parsed) != artifact_id or parsed.version != 4 or parsed.variant != uuid.RFC_4122:
        raise ValueError("artifact_id must be a canonical UUIDv4 string")
    return artifact_id


def _validate_output_section(section):
    if not isinstance(section, str) or section not in _OUTPUT_SECTIONS:
        raise ValueError("section must be one of: stdout, stderr, repr, traceback")
    return section


def _unavailable_output(artifact_id):
    return KeyError(
        f"Python output artifact {artifact_id} is unavailable because it was evicted or the runspace was reset"
    )


def _output_info(artifact_id):
    artifact_id = _validate_artifact_id(artifact_id)
    with _output_ring_lock:
        artifact = _output_ring.get(artifact_id)
        if artifact is None:
            raise _unavailable_output(artifact_id)
        sections = {
            name: {
                "captured_bytes": artifact["sections"][name]["captured_bytes"],
                "original_bytes": artifact["sections"][name]["original_bytes"],
                "excerpt_bytes": artifact["sections"][name]["excerpt_bytes"],
                "truncated": artifact["sections"][name]["truncated"],
            }
            for name in _OUTPUT_SECTIONS
        }
        retained_bytes = artifact["retained_bytes"]
        cell = artifact["cell"]
    return {
        "cell": cell,
        "artifact_id": artifact_id,
        "retained_bytes": retained_bytes,
        "max_read_bytes": MAX_OUTPUT_READ_BYTES,
        "sections": sections,
    }


def _output_read(artifact_id, section, offset, limit):
    artifact_id = _validate_artifact_id(artifact_id)
    section = _validate_output_section(section)
    if type(offset) is not int or offset < 0:
        raise TypeError("offset must be a non-negative integer byte offset")
    if type(limit) is not int or limit <= 0 or limit > MAX_OUTPUT_READ_BYTES:
        raise ValueError(
            f"limit must be an integer from 1 through {MAX_OUTPUT_READ_BYTES} bytes"
        )
    with _output_ring_lock:
        artifact = _output_ring.get(artifact_id)
        if artifact is None:
            raise _unavailable_output(artifact_id)
        value = artifact["sections"][section]["text"]
        cell = artifact["cell"]
    encoded = value.encode("utf-8")
    total = len(encoded)
    if offset > total:
        raise ValueError(f"offset exceeds the retained {section} length of {total} bytes")
    if offset < total and encoded[offset] & 0xC0 == 0x80:
        raise ValueError("offset must be zero or a next_offset returned by lethetic_output.read")
    end = min(total, offset + limit)
    while end < total and end > offset and encoded[end] & 0xC0 == 0x80:
        end -= 1
    if end == offset and offset < total:
        raise ValueError("limit is too small to include the next UTF-8 character")
    text = encoded[offset:end].decode("utf-8")
    return {
        "cell": cell,
        "artifact_id": artifact_id,
        "section": section,
        "offset": offset,
        "next_offset": end,
        "total_bytes": total,
        "eof": end == total,
        "text": text,
    }


_lethetic_output = types.ModuleType("lethetic_output")
_lethetic_output.__doc__ = (
    "Bounded access to output retained inside the current Lethetic Python runspace."
)
_lethetic_output.__all__ = ["info", "read"]
_lethetic_output.info = _output_info
_lethetic_output.read = _output_read
sys.modules["lethetic_output"] = _lethetic_output


def _store_output_artifact(cell, artifact_id, values, details):
    global _output_ring_bytes
    artifact_id = _validate_artifact_id(artifact_id)
    sections = {}
    retained_bytes = 0
    excerpts = {}
    metadata_sections = []
    for name in _OUTPUT_SECTIONS:
        excerpt, captured_bytes, excerpt_bytes = _result_excerpt(values[name], cell, name)
        excerpts[name] = excerpt
        retained_bytes += captured_bytes
        section = {
            "text": values[name],
            "captured_bytes": captured_bytes,
            "original_bytes": details[name]["original_bytes"],
            "excerpt_bytes": excerpt_bytes,
            "truncated": details[name]["truncated"],
        }
        sections[name] = section
        metadata_sections.append(
            {
                "section": name,
                "captured_bytes": captured_bytes,
                "original_bytes": section["original_bytes"],
                "excerpt_bytes": excerpt_bytes,
                "truncated": section["truncated"],
            }
        )
    artifact = {
        "cell": cell,
        "artifact_id": artifact_id,
        "retained_bytes": retained_bytes,
        "sections": sections,
    }
    retained = retained_bytes <= MAX_OUTPUT_RING_BYTES
    with _output_ring_lock:
        if retained:
            while _output_ring and (
                len(_output_ring) >= MAX_OUTPUT_ARTIFACTS
                or _output_ring_bytes + retained_bytes > MAX_OUTPUT_RING_BYTES
            ):
                _old_artifact_id, old = _output_ring.popitem(last=False)
                _output_ring_bytes -= old["retained_bytes"]
            _output_ring[artifact_id] = artifact
            _output_ring_bytes += retained_bytes
    return excerpts, {
        "cell": cell,
        "artifact_id": artifact_id,
        "retained": retained,
        "sections": metadata_sections,
    }


def _new_text_stream(fd):
    return os.fdopen(
        os.dup(fd),
        "w",
        buffering=1,
        encoding="utf-8",
        errors="backslashreplace",
        closefd=True,
    )


def _execute_cell(request_id, artifact_id, code):
    global _cell_number, _active_request_id, _host_sub_id
    artifact_id = _validate_artifact_id(artifact_id)
    _cell_number += 1
    with _host_call_lock:
        _active_request_id = request_id
        _host_sub_id = 0
    value_repr = ""
    error_traceback = ""
    ok = True
    cell_stdout = None
    cell_stderr = None
    stdout_capture = _BoundedPipeCapture(MAX_STREAM_BYTES)
    stderr_capture = _BoundedPipeCapture(MAX_STREAM_BYTES)

    try:
        stdout_capture.redirect_to(1)
        stderr_capture.redirect_to(2)
        cell_stdout = _new_text_stream(1)
        cell_stderr = _new_text_stream(2)
        _stdout_stream.set_target(cell_stdout)
        _stderr_stream.set_target(cell_stderr)
        sys.stdout = _stdout_stream
        sys.stderr = _stderr_stream
        sys.__stdout__ = _stdout_stream
        sys.__stderr__ = _stderr_stream
        sys.stdin = _blocked_input
        sys.__stdin__ = _blocked_input

        tree = ast.parse(code, filename=f"<lethetic-cell-{_cell_number}>", mode="exec")
        show_last = bool(tree.body) and isinstance(tree.body[-1], ast.Expr)
        if show_last:
            prefix = ast.Module(body=tree.body[:-1], type_ignores=[])
            ast.fix_missing_locations(prefix)
            if prefix.body:
                exec(
                    compile(prefix, f"<lethetic-cell-{_cell_number}>", "exec"),
                    _globals,
                    _globals,
                )
            expression = ast.Expression(body=tree.body[-1].value)
            ast.fix_missing_locations(expression)
            value = eval(
                compile(expression, f"<lethetic-cell-{_cell_number}>", "eval"),
                _globals,
                _globals,
            )
            _globals["_"] = value
            if value is not None and not code.rstrip().endswith(";"):
                value_repr = repr(value)
        else:
            exec(
                compile(tree, f"<lethetic-cell-{_cell_number}>", "exec"),
                _globals,
                _globals,
            )
    except BaseException:
        ok = False
        error_traceback = traceback.format_exc()
    finally:
        with _host_call_lock:
            _active_request_id = None
        for stream in (cell_stdout, cell_stderr):
            if stream is not None:
                try:
                    stream.flush()
                except BaseException:
                    pass
        os.dup2(_devnull_fd, 1)
        os.dup2(_devnull_fd, 2)
        _stdout_stream.set_target(None)
        _stderr_stream.set_target(None)
        for stream in (cell_stdout, cell_stderr):
            if stream is not None:
                try:
                    stream.close()
                except BaseException:
                    pass
        sys.stdout = _stdout_stream
        sys.stderr = _stderr_stream
        sys.__stdout__ = _stdout_stream
        sys.__stderr__ = _stderr_stream
        sys.stdin = _blocked_input
        sys.__stdin__ = _blocked_input

    stdout, stdout_truncated, stdout_bytes = stdout_capture.finish()
    stderr, stderr_truncated, stderr_bytes = stderr_capture.finish()
    value_repr, repr_truncated, repr_bytes = _bounded_string(value_repr, MAX_REPR_BYTES)
    error_traceback, traceback_truncated, traceback_bytes = _bounded_string(
        error_traceback, MAX_TRACEBACK_BYTES
    )
    values = {
        "stdout": stdout,
        "stderr": stderr,
        "repr": value_repr,
        "traceback": error_traceback,
    }
    details = {
        "stdout": {"truncated": stdout_truncated, "original_bytes": stdout_bytes},
        "stderr": {"truncated": stderr_truncated, "original_bytes": stderr_bytes},
        "repr": {"truncated": repr_truncated, "original_bytes": repr_bytes},
        "traceback": {
            "truncated": traceback_truncated,
            "original_bytes": traceback_bytes,
        },
    }
    excerpts, output_metadata = _store_output_artifact(
        _cell_number, artifact_id, values, details
    )
    try:
        cwd = os.getcwd()
    except BaseException:
        cwd = ""

    _send(
        {
            "type": "result",
            "id": request_id,
            "ok": ok,
            "cell": _cell_number,
            "stdout": excerpts["stdout"],
            "stderr": excerpts["stderr"],
            "repr": excerpts["repr"],
            "traceback": excerpts["traceback"],
            "cwd": cwd,
            "output_metadata": output_metadata,
        }
    )


os.dup2(_devnull_fd, 1)
os.dup2(_devnull_fd, 2)
sys.stdout = _stdout_stream
sys.stderr = _stderr_stream
sys.__stdout__ = _stdout_stream
sys.__stderr__ = _stderr_stream
sys.stdin = _blocked_input
sys.__stdin__ = _blocked_input
_send(
    {
        "type": "hello",
        "protocol": PROTOCOL_VERSION,
        "worker_abi": WORKER_ABI,
        "capabilities": [OUTPUT_RECOVERY_CAPABILITY],
        "python": sys.version.split()[0],
        "cwd": os.getcwd(),
    }
)

while True:
    raw = _control_input.readline(MAX_FRAME_BYTES + 2)
    if not raw:
        break
    try:
        if len(raw) > MAX_FRAME_BYTES + 1:
            raise RuntimeError("Python worker request exceeded its encoded frame limit")
        request = json.loads(raw)
        if not isinstance(request, dict):
            raise RuntimeError("Python worker request was not an object")
        request_id = request.get("id")
        operation = request.get("op")
        if operation == "execute":
            _execute_cell(
                request_id, request.get("artifact_id"), request.get("code", "")
            )
        elif operation == "shutdown":
            _send({"type": "shutdown", "id": request_id})
            break
        else:
            raise RuntimeError(f"Unknown runspace operation: {operation!r}")
    except BaseException:
        message, _truncated, _original = _bounded_string(
            traceback.format_exc(), MAX_TRACEBACK_BYTES
        )
        _send(
            {
                "type": "protocol_error",
                "id": None,
                "message": message,
            }
        )
