type Host = Window & {__LETHETIC_DESKTOP__?: boolean; __LETHETIC_HOST_TOKEN__?: string;
  __TAURI__?: {core: {invoke<T>(command: string, args: Record<string, unknown>): Promise<T>}}};
export function desktopDictationAvailable(): boolean {
  return typeof window !== "undefined" && window === window.top && (window as Host).__LETHETIC_DESKTOP__ === true &&
    typeof (window as Host).__LETHETIC_HOST_TOKEN__ === "string";
}
export async function dictationCommand<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!desktopDictationAvailable()) throw Error("Dictation requires the desktop app.");
  const host = window as Host;
  if (!host.__TAURI__) throw Error("Desktop microphone bridge unavailable. Update the app.");
  try { return await host.__TAURI__.core.invoke<T>(`plugin:lethetic-host|${command}`, {...args, token: host.__LETHETIC_HOST_TOKEN__}); }
  catch (error) { throw error instanceof Error ? error : Error(typeof error === "string" ? error : "Dictation failed"); }
}
