/** Native speech uses Rust HTTP; browser speech uses fetch and browser network policy. */
type HostWindow = Window & {
  __LETHETIC_HOST_TOKEN__?: string;
  __TAURI__?: { core: { invoke<T>(command: string, args: Record<string, unknown>): Promise<T> } };
};
function failure(error: unknown): Error { return error instanceof Error ? error : new Error(typeof error === "string" ? error : "Native speech request failed"); }
type SpeechEvent = {kind: "head"; status: number; headers: Record<string, string>} | {kind: "chunk"; data: string} | {kind: "done"} | {kind: "error"; message: string};
export async function fetchSpeech(url: string, init: RequestInit, signal: AbortSignal): Promise<Response> {
  const host = typeof window === "undefined" ? undefined : window as HostWindow;
  if (!host || host !== host.top || typeof host.__LETHETIC_HOST_TOKEN__ !== "string") return fetch(url, {...init, signal});
  if (!host.__TAURI__) throw Error("Native speech bridge unavailable. Update the executable app.");
  const bridge = host.__TAURI__.core, token = host.__LETHETIC_HOST_TOKEN__, id = crypto.randomUUID();
  const invoke = <T>(command: string, args: Record<string, unknown> = {}): Promise<T> => bridge.invoke<T>(`plugin:lethetic-host|${command}`, {...args, id, token});
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null, finished = false;
  const cancel = (): void => { void invoke("cancel_speech").catch(() => {}); };
  const cleanup = (): void => { finished = true; signal.removeEventListener("abort", abort); cancel(); };
  const abort = (): void => {
    if (controller && !finished) controller.error(signal.reason);
    cleanup();
  };
  signal.throwIfAborted(); signal.addEventListener("abort", abort, {once: true});
  try {
    await invoke("begin_speech", {request: {url, headers: Object.fromEntries(new Headers(init.headers)), body: String(init.body ?? "")}});
    // Also cancel after start returns: an abort can race ahead of native registration.
    signal.throwIfAborted();
    const head = await invoke<SpeechEvent>("next_speech"); signal.throwIfAborted();
    if (head.kind === "error") throw Error(head.message);
    if (head.kind !== "head") throw Error("Native speech response headers missing");
    if ([204, 205, 304].includes(head.status)) { cleanup(); return new Response(null, {status: head.status, headers: head.headers}); }
    const body = new ReadableStream<Uint8Array>({
      start(value) { controller = value; },
      async pull(value) {
        try {
          const event = await invoke<SpeechEvent>("next_speech");
          if (finished) return;
          if (event.kind === "done") { value.close(); cleanup(); }
          else if (event.kind === "chunk") {
            if (event.data.length > 90_000) throw Error("Native speech chunk too large");
            value.enqueue(Uint8Array.from(atob(event.data), c => c.charCodeAt(0)));
          } else { throw Error(event.kind === "error" ? event.message : "Unexpected native speech response"); }
        } catch (error) { if (!finished) { value.error(failure(error)); cleanup(); } }
      },
      cancel() { cleanup(); },
    });
    return new Response(body, {status: head.status, headers: head.headers});
  } catch (error) { cleanup(); signal.throwIfAborted(); throw failure(error); }
}
