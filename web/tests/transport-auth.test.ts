import { TEST_ORIGIN as origin } from "./support/transport-globals.js";
import assert from "node:assert/strict";
import test from "node:test";
import type { ICommandRequest } from "../src/generated/contracts.js";
import { classifyBootstrapFragment } from "../src/safety.js";
import {
  BrowserTransport,
  type BrowserTransportEvents,
  type TransportStatus,
} from "../src/transport.js";
import { nth } from "./support/collections.js";
import { isRecord } from "./support/vnode.js";

const firstProof = `lethetic-wfe-v1.${"A".repeat(43)}`;
const secondProof = `lethetic-wfe-v1.${"B".repeat(43)}`;

type FetchInput = RequestInfo | URL;

/** The subset of a fetch Response that the transport reads. */
interface FakeResponse {
  readonly ok: boolean;
  readonly status: number;
  readonly headers?: Headers;
  readonly body?: { cancel(): Promise<void> } | null;
}

interface FetchCall {
  readonly input: FetchInput;
  readonly init: RequestInit | undefined;
}

type FakeFetch = (input: FetchInput, init?: RequestInit) => Promise<FakeResponse>;

/** Install a fetch double; its partial responses are not real Response objects. */
function installFetch(fetch: FakeFetch): void {
  Reflect.set(globalThis, "fetch", fetch);
}

function requestJson(init: RequestInit | undefined): unknown {
  const body = init?.body;
  assert.ok(typeof body === "string", "request body is not a string");
  return JSON.parse(body);
}

function sessionResponse(proof: string): FakeResponse {
  return {
    ok: true,
    status: 204,
    headers: new Headers({
      "X-Lethetic-WebSocket-Protocol": proof,
    }),
  };
}

function events(overrides: Partial<BrowserTransportEvents> = {}): BrowserTransportEvents {
  return {
    socketOpened() {},
    authenticationEpochChanged() {},
    serverMessage() {},
    commandResponse() {},
    transportStatus() {},
    protocolFault() {},
    ...overrides,
  };
}

function hasFileErrorCode(code: string): (error: unknown) => boolean {
  return (error) =>
    isRecord(error) && error["name"] === "FileServiceError" && error["code"] === code;
}

interface FakeTimer {
  readonly callback: () => void;
  readonly deadline: number;
}

class FakeTimers {
  #nextId = 1;
  #now = 0;
  readonly #timers = new Map<number, FakeTimer>();

  setTimeout(callback: TimerHandler, delay: number | undefined = 0): number {
    if (typeof callback !== "function") {
      throw new Error("string timer handlers are not supported");
    }
    const id = this.#nextId;
    this.#nextId += 1;
    const normalizedDelay = Math.max(0, Number(delay));
    this.#timers.set(id, {
      callback: () => {
        callback();
      },
      deadline: this.#now + normalizedDelay,
    });
    return id;
  }

  clearTimeout(id: number | undefined): void {
    if (id !== undefined) {
      this.#timers.delete(id);
    }
  }

  delays(): number[] {
    return Array.from(
      this.#timers.values(),
      ({ deadline }) => deadline - this.#now,
    ).sort((left, right) => left - right);
  }

  async #flushMicrotasks(): Promise<void> {
    for (let index = 0; index < 8; index += 1) {
      await Promise.resolve();
    }
  }

  #nextTimer(): [number, FakeTimer] | undefined {
    return Array.from(this.#timers.entries()).sort(
      ([leftId, left], [rightId, right]) =>
        left.deadline - right.deadline || leftId - rightId,
    )[0];
  }

  async advanceBy(milliseconds: number): Promise<void> {
    const target = this.#now + milliseconds;
    while (true) {
      const next = this.#nextTimer();
      if (next === undefined || next[1].deadline > target) {
        break;
      }
      const [id, timer] = next;
      this.#timers.delete(id);
      this.#now = timer.deadline;
      timer.callback();
      await this.#flushMicrotasks();
    }
    this.#now = target;
    await this.#flushMicrotasks();
  }

  async runNext(): Promise<number> {
    const next = this.#nextTimer();
    assert.ok(next !== undefined, "no fake timer was scheduled");
    const delay = next[1].deadline - this.#now;
    await this.advanceBy(delay);
    return delay;
  }
}

type EventCallback = (event: unknown) => void;

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;

  readyState: number = FakeWebSocket.CONNECTING;
  bufferedAmount = 0;
  readonly sent: string[] = [];
  closeCode: number | null = null;
  closeReason: string | null = null;
  readonly url: string;
  readonly protocol: string | readonly string[] | undefined;
  readonly #listeners = new Map<string, EventCallback[]>();

  constructor(url: string | URL, protocol?: string | readonly string[]) {
    this.url = String(url);
    this.protocol = protocol;
    FakeWebSocket.created?.push(this);
  }

  /** Receives each constructed socket while a fake browser is installed. */
  static created: FakeWebSocket[] | null = null;

  addEventListener(type: string, callback: EventCallback): void {
    const listeners = this.#listeners.get(type) ?? [];
    listeners.push(callback);
    this.#listeners.set(type, listeners);
  }

  send(value: string): void {
    this.sent.push(value);
  }

  close(code: number | null = null, reason: string | null = null): void {
    this.closeCode = code;
    this.closeReason = reason;
    this.readyState = FakeWebSocket.CLOSING;
  }

  open(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.#emit("open", {});
  }

  serverClose(): void {
    this.readyState = FakeWebSocket.CLOSED;
    this.#emit("close", {});
  }

  #emit(type: string, event: unknown): void {
    for (const listener of this.#listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

interface FakeNavigator {
  onLine: boolean;
}

interface FakeBrowser {
  readonly timers: FakeTimers;
  readonly sockets: FakeWebSocket[];
  readonly navigator: FakeNavigator;
  readonly dispatch: (type: string, event?: unknown) => void;
}

async function withFakeBrowser(run: (browser: FakeBrowser) => Promise<void>): Promise<void> {
  const originalFetch = globalThis.fetch;
  const originalWebSocket = globalThis.WebSocket;
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  const originalAddEventListener = globalThis.addEventListener;
  const navigatorDescriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "navigator",
  );
  const timers = new FakeTimers();
  const sockets: FakeWebSocket[] = [];
  const listeners = new Map<string, EventCallback[]>();
  const navigatorState: FakeNavigator = { onLine: true };

  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: navigatorState,
  });
  FakeWebSocket.created = sockets;
  // The double implements only the WebSocket surface the transport uses.
  Reflect.set(globalThis, "WebSocket", FakeWebSocket);
  globalThis.setTimeout = (callback, delay) =>
    timers.setTimeout(callback, delay);
  globalThis.clearTimeout = (id) => timers.clearTimeout(id);
  globalThis.addEventListener = (type: string, callback: unknown) => {
    if (typeof callback !== "function") {
      throw new Error("listener objects are not supported");
    }
    const registered = listeners.get(type) ?? [];
    registered.push((event) => {
      callback(event);
    });
    listeners.set(type, registered);
  };
  const dispatch = (type: string, event: unknown = {}): void => {
    for (const listener of listeners.get(type) ?? []) {
      listener(event);
    }
  };

  try {
    await run({
      timers,
      sockets,
      navigator: navigatorState,
      dispatch,
    });
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.WebSocket = originalWebSocket;
    FakeWebSocket.created = null;
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
    globalThis.addEventListener = originalAddEventListener;
    if (navigatorDescriptor === undefined) {
      Reflect.deleteProperty(globalThis, "navigator");
    } else {
      Object.defineProperty(globalThis, "navigator", navigatorDescriptor);
    }
  }
}

test("authentication uses browser CORS mode under no-referrer", async () => {
  await withFakeBrowser(async () => {
    const seen: { call: FetchCall | null } = { call: null };
    installFetch(async (input, init) => {
      seen.call = { input, init };
      return {
        ok: false,
        status: 401,
        headers: new Headers(),
      };
    });

    const statuses: TransportStatus[] = [];
    const transport = new BrowserTransport(
      events({
        transportStatus(status) {
          statuses.push(status);
        },
      }),
    );

    await transport.authenticate("test-controller-token");
    const captured = seen.call;
    assert.ok(captured !== null);
    assert.equal(captured.input, "/auth");
    assert.equal(captured.init?.method, "POST");
    assert.equal(captured.init?.mode, "cors");
    assert.equal(captured.init?.credentials, "same-origin");
    assert.equal(captured.init?.cache, "no-store");
    assert.equal(captured.init?.redirect, "error");
    assert.equal(captured.init?.referrerPolicy, "no-referrer");
    assert.deepEqual(captured.init?.headers, {
      "Content-Type": "application/json",
    });
    assert.deepEqual(requestJson(captured.init), {
      token: "test-controller-token",
    });
    assert.equal(
      Object.keys(captured.init?.headers ?? {}).some(
        (name) => name.toLocaleLowerCase() === "origin",
      ),
      false,
    );
    assert.equal(statuses.at(-1)?.phase, "auth_failed");
  });
});

test("file requests require synchronization and the negotiated read-only capability", async () => {
  await withFakeBrowser(async ({ sockets }) => {
    const calls: FetchCall[] = [];
    const fileResponse: FakeResponse = { ok: true, status: 200, body: null };
    installFetch(async (input, init) => {
      calls.push({ input, init });
      if (input === "/auth/session") {
        return sessionResponse(firstProof);
      }
      return fileResponse;
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    const socket = nth(sockets, 0);
    socket.open();
    const signal = new AbortController().signal;

    transport.noteHello(true);
    await assert.rejects(
      transport.requestFile("/api/files/list", "", signal),
      hasFileErrorCode("unavailable"),
    );
    assert.equal(calls.length, 1);

    transport.noteHello();
    transport.markSynchronized();
    await assert.rejects(
      transport.requestFile("/api/files/list", "", signal),
      hasFileErrorCode("unavailable"),
    );
    assert.equal(calls.length, 1);

    transport.noteHello(true);
    assert.equal(
      await transport.requestFile("/api/files/list", "", signal),
      fileResponse,
    );
    assert.equal(calls.length, 2);

    transport.markStale();
    await assert.rejects(
      transport.requestFile("/api/files/list", "", signal),
      hasFileErrorCode("unavailable"),
    );
    assert.equal(calls.length, 2);
  });
});

test("file requests use only exact endpoints, bounded paths, and the connection proof header", async () => {
  await withFakeBrowser(async ({ sockets }) => {
    const calls: FetchCall[] = [];
    installFetch(async (input, init) => {
      if (input === "/auth/session") {
        return sessionResponse(firstProof);
      }
      calls.push({ input, init });
      return { ok: true, status: 200, body: null };
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    nth(sockets, 0).open();
    transport.noteHello(true);
    transport.markSynchronized();

    const cases = [
      ["/api/files/list", ""],
      ["/api/files/read", "notes/naïve file.txt"],
      ["/api/files/download", "artifacts/result.bin"],
      ["/api/files/archive", "artifacts"],
    ] as const;
    for (const [endpoint, path] of cases) {
      const signal = new AbortController().signal;
      await transport.requestFile(endpoint, path, signal);
      const call = nth(calls, -1);
      assert.equal(call.input, endpoint);
      assert.equal(call.init?.method, "POST");
      assert.equal(call.init?.credentials, "same-origin");
      assert.equal(call.init?.mode, "cors");
      assert.equal(call.init?.cache, "no-store");
      assert.equal(call.init?.redirect, "error");
      assert.equal(call.init?.referrerPolicy, "no-referrer");
      assert.equal(call.init?.signal, signal);
      assert.deepEqual(call.init?.headers, {
        "Content-Type": "application/json",
        "X-Lethetic-WebSocket-Protocol": firstProof,
      });
      assert.equal(String(call.input).includes(firstProof), false);
      assert.deepEqual(requestJson(call.init), { path });
      assert.equal(String(call.init?.body).includes(firstProof), false);
    }
    assert.equal(calls.length, cases.length);

    // Deliberately outside the FileEndpoint union: the transport must reject them.
    const invalidEndpoints: readonly string[] = [
      "/api/files/list?path=notes",
      "/api/files/READ",
      "/api/files/delete",
      `${origin}/api/files/read`,
    ];
    for (const endpoint of invalidEndpoints) {
      await assert.rejects(
        transport.requestFile(
          endpoint as Parameters<BrowserTransport["requestFile"]>[0],
          "notes/file.txt",
          new AbortController().signal,
        ),
        hasFileErrorCode("bad_request"),
      );
    }

    const invalidPaths = [
      "",
      "/absolute",
      "../secret",
      "notes/../secret",
      "notes/./file",
      "notes//file",
      "notes\\file",
      `notes${String.fromCharCode(0)}file`,
      "C:secret",
      "é".repeat(2049),
      Array.from({ length: 65 }, () => "segment").join("/"),
      "\"".repeat(4096),
    ];
    for (const path of invalidPaths) {
      await assert.rejects(
        transport.requestFile(
          "/api/files/read",
          path,
          new AbortController().signal,
        ),
        hasFileErrorCode("bad_request"),
      );
    }
    assert.equal(calls.length, cases.length);
  });
});

test("a fresh authentication epoch cancels a late file response", async () => {
  await withFakeBrowser(async ({ sockets }) => {
    let authenticationCount = 0;
    const pendingFile: { resolve: ((response: FakeResponse) => void) | null } = { resolve: null };
    let fileBodyCancellations = 0;
    installFetch(async (input) => {
      if (input === "/auth/session") {
        authenticationCount += 1;
        return sessionResponse(authenticationCount === 1 ? firstProof : secondProof);
      }
      return new Promise<FakeResponse>((resolveResponse) => {
        pendingFile.resolve = resolveResponse;
      });
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    nth(sockets, 0).open();
    transport.noteHello(true);
    transport.markSynchronized();

    const fileRequest = transport.requestFile(
      "/api/files/read",
      "notes/file.txt",
      new AbortController().signal,
    );
    const resolveFile = pendingFile.resolve;
    assert.ok(resolveFile !== null);

    await transport.authenticateSession();
    assert.equal(authenticationCount, 2);
    assert.equal(nth(sockets, -1).protocol, secondProof);

    resolveFile({
      ok: true,
      status: 200,
      body: {
        async cancel() {
          fileBodyCancellations += 1;
        },
      },
    });
    await assert.rejects(fileRequest, hasFileErrorCode("cancelled"));
    assert.equal(fileBodyCancellations, 1);
  });
});

test("empty bootstrap is tokenless while malformed fragments never downgrade", () => {
  assert.deepEqual(classifyBootstrapFragment(""), { type: "session" });
  assert.deepEqual(classifyBootstrapFragment("#token=abc_DEF-123"), {
    type: "token",
    token: "abc_DEF-123",
  });
  for (const fragment of ["#", "#token=", "#bad=value", "not?valid"]) {
    assert.deepEqual(classifyBootstrapFragment(fragment), { type: "malformed" });
  }
});

test("tokenless authentication posts an exact empty object and honors Retry-After", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    const calls: FetchCall[] = [];
    installFetch(async (input, init) => {
      calls.push({ input, init });
      if (calls.length === 1) {
        return {
          ok: false,
          status: 429,
          headers: new Headers({ "Retry-After": "60" }),
        };
      }
      return sessionResponse(firstProof);
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    assert.equal(calls.length, 1);
    assert.equal(nth(calls, 0).input, "/auth/session");
    assert.deepEqual(requestJson(nth(calls, 0).init), {});
    assert.deepEqual(timers.delays(), [60_000]);
    assert.equal(sockets.length, 0);

    assert.equal(await timers.runNext(), 60_000);
    assert.equal(calls.length, 2);
    assert.equal(sockets.length, 1);
    assert.equal(nth(sockets, 0).protocol, firstProof);
    assert.equal(nth(sockets, 0).url, `${origin.replace("https:", "wss:")}/ws`);
  });
});

test("pre-open failure reauthenticates once and never replays pending commands across epochs", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    const fetchCalls: FetchCall[] = [];
    installFetch(async (input, init) => {
      fetchCalls.push({ input, init });
      return sessionResponse(fetchCalls.length === 1 ? firstProof : secondProof);
    });
    const abandoned: ICommandRequest[][] = [];
    const transport = new BrowserTransport(
      events({
        authenticationEpochChanged(requests) {
          abandoned.push([...requests]);
        },
      }),
    );

    await transport.authenticateSession();
    assert.equal(fetchCalls.length, 1);
    assert.equal(sockets.length, 1);
    const first = nth(sockets, 0);
    first.open();
    transport.noteHello();
    transport.markSynchronized();
    const request = transport.send({ type: "request_snapshot" }, 0);
    assert.ok(request !== null);
    assert.equal(first.sent.length, 1);
    assert.equal(transport.pendingCount, 1);

    first.serverClose();
    assert.equal(await timers.runNext(), 400);
    assert.equal(sockets.length, 2);
    const staleProofSocket = nth(sockets, 1);
    assert.equal(staleProofSocket.protocol, firstProof);
    staleProofSocket.serverClose();
    staleProofSocket.serverClose();
    assert.equal(timers.delays().length, 1);
    assert.equal(await timers.runNext(), 800);

    assert.equal(fetchCalls.length, 2);
    assert.equal(nth(fetchCalls, 1).input, "/auth/session");
    assert.equal(sockets.length, 3);
    const refreshed = nth(sockets, 2);
    assert.equal(refreshed.protocol, secondProof);
    assert.equal(abandoned.length, 1);
    assert.deepEqual(nth(abandoned, 0).map(({ id }) => id), [request.id]);
    assert.equal(transport.pendingCount, 0);

    refreshed.open();
    transport.noteHello();
    transport.markSynchronized();
    assert.deepEqual(refreshed.sent, []);
    assert.equal(fetchCalls.length, 2);
  });
});

test("token authentication retries retryable bootstrap failures", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    const calls: FetchCall[] = [];
    installFetch(async (input, init) => {
      calls.push({ input, init });
      if (calls.length === 1) {
        return {
          ok: false,
          status: 429,
          headers: new Headers({ "Retry-After": "1" }),
        };
      }
      return sessionResponse(firstProof);
    });

    const transport = new BrowserTransport(events());
    await transport.authenticate("retryable-controller-token");
    assert.equal(calls.length, 1);
    assert.deepEqual(timers.delays(), [1_000]);

    assert.equal(await timers.runNext(), 1_000);
    assert.equal(calls.length, 2);
    assert.equal(nth(calls, 1).input, "/auth");
    assert.deepEqual(requestJson(nth(calls, 1).init), {
      token: "retryable-controller-token",
    });
    assert.equal(sockets.length, 1);
    assert.equal(nth(sockets, 0).protocol, firstProof);
  });
});

test("offline and online events cannot bypass Retry-After", async () => {
  await withFakeBrowser(async ({ timers, sockets, navigator, dispatch }) => {
    let calls = 0;
    installFetch(async () => {
      calls += 1;
      if (calls === 1) {
        return {
          ok: false,
          status: 429,
          headers: new Headers({ "Retry-After": "60" }),
        };
      }
      return sessionResponse(firstProof);
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    navigator.onLine = false;
    dispatch("offline");
    navigator.onLine = true;
    dispatch("online");

    assert.equal(calls, 1);
    assert.deepEqual(timers.delays(), [60_000]);
    await timers.advanceBy(59_999);
    assert.equal(calls, 1);
    assert.equal(sockets.length, 0);
    await timers.advanceBy(1);
    assert.equal(calls, 2);
    assert.equal(sockets.length, 1);
  });
});

test("authentication is aborted at its client-side deadline", async () => {
  await withFakeBrowser(async ({ timers }) => {
    const seen: { signal: AbortSignal | null } = { signal: null };
    installFetch(async (_input, init) =>
      new Promise<FakeResponse>((_resolve, reject) => {
        const signal = init?.signal;
        assert.ok(signal);
        seen.signal = signal;
        signal.addEventListener("abort", () => {
          reject(new DOMException("aborted", "AbortError"));
        });
      }));

    const transport = new BrowserTransport(events());
    const authentication = transport.authenticateSession();
    assert.deepEqual(timers.delays(), [10_000]);
    await timers.advanceBy(10_000);
    await authentication;

    const signal = seen.signal;
    assert.ok(signal !== null);
    assert.equal(signal.aborted, true);
    assert.deepEqual(timers.delays(), [400]);
    transport.closePermanently("test complete");
    assert.deepEqual(timers.delays(), []);
  });
});

test("online events cannot create a socket during session authentication", async () => {
  await withFakeBrowser(async ({ timers, sockets, dispatch }) => {
    let calls = 0;
    const pendingRefresh: { resolve: ((response: FakeResponse) => void) | null } = { resolve: null };
    installFetch(async () => {
      calls += 1;
      if (calls === 1) {
        return sessionResponse(firstProof);
      }
      return new Promise<FakeResponse>((resolveResponse) => {
        pendingRefresh.resolve = resolveResponse;
      });
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    assert.equal(sockets.length, 1);
    nth(sockets, 0).open();
    nth(sockets, 0).serverClose();
    assert.equal(await timers.runNext(), 400);
    assert.equal(calls, 1);
    assert.equal(sockets.length, 2);
    nth(sockets, 1).serverClose();
    assert.equal(await timers.runNext(), 800);
    assert.equal(calls, 2);

    dispatch("online");
    assert.equal(sockets.length, 2);
    const resolveRefresh = pendingRefresh.resolve;
    assert.ok(resolveRefresh !== null);
    resolveRefresh(sessionResponse(secondProof));
    for (let index = 0; index < 8; index += 1) {
      await Promise.resolve();
    }

    assert.equal(sockets.length, 3);
    assert.equal(nth(sockets, 2).protocol, secondProof);
  });
});

test("liveness response deadline is not postponed by synchronization traffic", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    installFetch(async () => sessionResponse(firstProof));

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    const socket = nth(sockets, 0);
    socket.open();
    transport.noteHello();
    transport.markSynchronized();

    assert.equal(await timers.runNext(), 20_000);
    assert.equal(socket.sent.length, 1);
    await timers.advanceBy(9_000);
    transport.markSynchronized();
    transport.markSynchronized();
    await timers.advanceBy(1_000);

    assert.equal(socket.readyState, WebSocket.CLOSING);
    assert.equal(socket.closeCode, 1011);
    assert.equal(socket.closeReason, "liveness response timeout");
  });
});

test("stale state has a hard recovery deadline", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    installFetch(async () => sessionResponse(firstProof));

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    const socket = nth(sockets, 0);
    socket.open();
    transport.noteHello();
    transport.markSynchronized();
    transport.markStale();
    await timers.advanceBy(9_000);
    transport.markStale();
    await timers.advanceBy(1_000);

    assert.equal(socket.readyState, WebSocket.CLOSING);
    assert.equal(socket.closeCode, 1011);
    assert.equal(socket.closeReason, "state recovery timeout");
  });
});

test("BFCache restoration creates a fresh socket without discarding its proof", async () => {
  await withFakeBrowser(async ({ sockets, dispatch }) => {
    installFetch(async () => sessionResponse(firstProof));

    const statuses: TransportStatus[] = [];
    const transport = new BrowserTransport(
      events({
        transportStatus(status) {
          statuses.push(status);
        },
      }),
    );
    await transport.authenticateSession();
    const original = nth(sockets, 0);
    original.open();
    transport.noteHello();
    transport.markSynchronized();

    dispatch("pagehide", { persisted: true });
    assert.equal(original.readyState, WebSocket.CLOSING);
    assert.equal(statuses.at(-1)?.phase, "offline");
    dispatch("pageshow", { persisted: true });

    assert.equal(sockets.length, 2);
    assert.equal(nth(sockets, 1).protocol, firstProof);
    assert.equal(statuses.at(-1)?.phase, "reconnecting");
  });
});

test("same-process session refresh preserves replay state", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    let fetchCalls = 0;
    installFetch(async () => {
      fetchCalls += 1;
      return sessionResponse(firstProof);
    });

    const abandoned: ICommandRequest[][] = [];
    const transport = new BrowserTransport(
      events({
        authenticationEpochChanged(requests) {
          abandoned.push([...requests]);
        },
      }),
    );
    await transport.authenticateSession();
    const original = nth(sockets, 0);
    original.open();
    transport.noteHello();
    transport.markSynchronized();
    const request = transport.send({ type: "request_snapshot" }, 0);
    assert.ok(request !== null);

    original.serverClose();
    assert.equal(await timers.runNext(), 400);
    nth(sockets, 1).serverClose();
    assert.equal(await timers.runNext(), 800);

    assert.equal(fetchCalls, 2);
    assert.equal(sockets.length, 3);
    assert.deepEqual(abandoned, []);
    assert.equal(transport.pendingCount, 1);
    const refreshed = nth(sockets, 2);
    refreshed.open();
    transport.noteHello();
    transport.markSynchronized();
    assert.equal(refreshed.sent.length, 1);
    const replayed: unknown = JSON.parse(nth(refreshed.sent, 0));
    assert.ok(isRecord(replayed));
    assert.equal(replayed["id"], request.id);
  });
});

test("fresh session proof survives bounded pre-open admission failures", async () => {
  await withFakeBrowser(async ({ timers, sockets }) => {
    let fetchCalls = 0;
    installFetch(async () => {
      fetchCalls += 1;
      return sessionResponse(firstProof);
    });

    const transport = new BrowserTransport(events());
    await transport.authenticateSession();
    for (const expectedDelay of [400, 800, 1_600, 3_200]) {
      nth(sockets, -1).serverClose();
      assert.equal(await timers.runNext(), expectedDelay);
      assert.equal(fetchCalls, 1);
      assert.equal(nth(sockets, -1).protocol, firstProof);
    }

    nth(sockets, -1).serverClose();
    assert.equal(await timers.runNext(), 6_400);
    assert.equal(fetchCalls, 2);
    assert.equal(sockets.length, 6);
    assert.equal(nth(sockets, -1).protocol, firstProof);
  });
});
