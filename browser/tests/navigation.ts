type Message = Record<string, unknown>;
type Rpc = (method: string, params?: Message) => Promise<Message>;
export type CdpListener = (message: Message) => void;

/** Page.navigate/reload acknowledge the command before the new document is ready. */
export async function navigateDocument(
  rpc: Rpc,
  subscribe: (listener: CdpListener) => () => void,
  sessionId: string,
  method: "Page.navigate" | "Page.reload",
  params: Message = {},
  timeoutMs = 30_000,
): Promise<void> {
  let frameId: string | undefined;
  let loaderId: string | undefined;
  let unsubscribe = (): void => {};
  let timer: ReturnType<typeof setTimeout> | undefined;
  const ready = new Promise<void>((resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`${method}: new main document did not reach DOMContentLoaded`)), timeoutMs);
    // Subscribe before issuing the command: fast navigations can finish before
    // the command response, and old-document/subframe events must not release us.
    unsubscribe = subscribe(message => {
      if (message["sessionId"] !== sessionId) return;
      const event = message["params"] as Message | undefined;
      if (message["method"] === "Page.frameNavigated") {
        const frame = event?.["frame"] as Message | undefined;
        if (frame && !frame["parentId"] && typeof frame["id"] === "string" && typeof frame["loaderId"] === "string") {
          frameId = frame["id"]; loaderId = frame["loaderId"];
        }
      } else if (message["method"] === "Page.lifecycleEvent" && loaderId &&
          event?.["name"] === "DOMContentLoaded" && event["frameId"] === frameId && event["loaderId"] === loaderId) {
        resolve();
      }
    });
  });
  try {
    await Promise.all([ready, rpc(method, params).then(response => {
      if (response["errorText"]) throw new Error(`${method}: ${String(response["errorText"])}`);
    })]);
  } finally {
    clearTimeout(timer);
    unsubscribe();
  }
}
