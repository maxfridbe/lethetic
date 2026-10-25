/** Save through the OS picker in the native app, or a normal download in browsers. */
type NativeWindow = Window & {
  __LETHETIC_HOST_TOKEN__?: string;
  __TAURI__?: { core: { invoke<T>(command: string, args: Record<string, unknown>): Promise<T> } };
};
const CHUNK_BYTES = 256 * 1024;
let nativeBusy = false;

export async function downloadBlob(blob: Blob, filename: string, signal?: AbortSignal): Promise<boolean> {
  const name = filename.replace(/[\\/\u0000-\u001f\u007f]/gu, "_").slice(0, 180) || "download";
  signal?.throwIfAborted();
  const host = typeof window === "undefined" ? undefined : window as NativeWindow;
  if (host && host === host.top && typeof host.__LETHETIC_HOST_TOKEN__ === "string") {
    if (!host.__TAURI__) throw new Error("Native save dialog is unavailable");
    if (nativeBusy) throw new Error("Finish or cancel the current download first");
    const bridge = host.__TAURI__.core;
    const token = host.__LETHETIC_HOST_TOKEN__;
    const invoke = <T>(command: string, args: Record<string, unknown>) =>
      bridge.invoke<T>(`plugin:lethetic-host|${command}`, { ...args, token });
    nativeBusy = true;
    let id: string | null = null;
    try {
      id = await invoke<string>("begin_download", { name, mime: blob.type || "application/octet-stream", size: blob.size });
      for (let offset = 0; offset < blob.size; offset += CHUNK_BYTES) {
        signal?.throwIfAborted();
        const bytes = new Uint8Array(await blob.slice(offset, offset + CHUNK_BYTES).arrayBuffer());
        let binary = "";
        for (let start = 0; start < bytes.length; start += 8192) binary += String.fromCharCode(...bytes.subarray(start, start + 8192));
        await invoke<void>("write_download", { id, offset, data: btoa(binary) });
      }
      signal?.throwIfAborted();
      return await invoke<boolean>("finish_download", { id });
    } finally {
      if (id !== null) await invoke<void>("cancel_download", { id }).catch(() => {});
      nativeBusy = false;
    }
  }
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url; anchor.download = name; anchor.rel = "noopener"; anchor.hidden = true;
  document.body.append(anchor);
  try { anchor.click(); } finally { anchor.remove(); setTimeout(() => URL.revokeObjectURL(url), 60_000); }
  return true;
}
