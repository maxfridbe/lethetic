import type * as Monaco from "../../lib/monaco/vs/editor/editor.api.js";
import type { FilesReadResponse, GitDiffResponse } from "../generated/contracts.js";

type MonacoApi = typeof Monaco;
let assets: Promise<MonacoApi> | null = null;

async function loadMonaco(): Promise<MonacoApi> {
  if (assets !== null) return assets;
  assets = (async () => {
    const environment = globalThis as typeof globalThis & {
      MonacoEnvironment?: { getWorker: (_module: string, label: string) => Worker };
    };
    environment.MonacoEnvironment = {
      getWorker: (_module, label) => {
        if (label !== "editorWorkerService") throw new Error("Unsupported editor worker");
        const url = new URL("../../lib/monaco/vs/editor/editor.worker.js", import.meta.url);
        if (url.origin !== globalThis.location.origin || url.protocol !== "https:") {
          throw new Error("A same-origin HTTPS editor worker is required");
        }
        return new Worker(url, { type: "module", name: "lethetic-file-editor" });
      },
    };
    const css = globalThis.document.createElement("link");
    css.rel = "stylesheet";
    css.href = new URL("../../lib/monaco/monaco.css", import.meta.url).href;
    const styled = new Promise<void>((resolve, reject) => {
      css.addEventListener("load", () => resolve(), { once: true });
      css.addEventListener("error", () => reject(new Error("Editor styles unavailable")), { once: true });
    });
    globalThis.document.head.append(css);
    const api = await import("../../lib/monaco/vs/editor/editor.api.js");
    await Promise.all([
      styled,
      import("../../lib/monaco/vs/basic-languages/monaco.contribution.js"),
      import("../../lib/monaco/vs/editor/contrib/find/browser/findController.js"),
    ]);
    return api;
  })();
  return assets;
}

export function fileLanguage(path: string, languages: readonly Monaco.languages.ILanguageExtensionPoint[]): string {
  const name = path.split("/").at(-1) ?? "";
  return languages.find((language) => language.filenames?.includes(name))?.id ??
    languages.find((language) => language.extensions?.some((extension) => name.endsWith(extension)))?.id ??
    "plaintext";
}

export class MonacoFileEditor {
  #host: HTMLElement | null = null;
  #api: MonacoApi | null = null;
  #editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  #model: Monaco.editor.ITextModel | null = null;
  #preview: FilesReadResponse | null = null;
  #applied: FilesReadResponse | null = null;
  #observer: ResizeObserver | null = null;
  #generation = 0;

  constructor(readonly failed: () => void, readonly resized: () => void) {}

  sync(host: HTMLElement | null, preview: FilesReadResponse | null): void {
    if (host === this.#host) {
      this.#preview = preview;
      this.#apply();
      return;
    }
    this.dispose();
    if (host === null) return;
    this.#host = host;
    this.#preview = preview;
    const generation = this.#generation;
    void loadMonaco().then((api) => {
      if (this.#generation !== generation || this.#host !== host || !host.isConnected) return;
      this.#api = api;
      this.#editor = api.editor.create(host, {
        model: null, readOnly: true, domReadOnly: true, theme: "vs-dark",
        automaticLayout: false, ariaLabel: "Read-only file preview",
        minimap: { enabled: false }, links: false, contextmenu: false,
        folding: true, glyphMargin: false, lineNumbers: "on", wordWrap: "off",
        scrollBeyondLastLine: false, fontSize: 13, fontFamily: "monospace",
        quickSuggestions: false, suggestOnTriggerCharacters: false,
        parameterHints: { enabled: false }, wordBasedSuggestions: "off",
        occurrencesHighlight: "off", selectionHighlight: false,
        codeLens: false, colorDecorators: false, lightbulb: { enabled: api.editor.ShowLightbulbIconMode.Off },
        stickyScroll: { enabled: false }, renderValidationDecorations: "off",
        unicodeHighlight: { ambiguousCharacters: false, invisibleCharacters: false },
        bracketPairColorization: { enabled: false },
        padding: { top: 8, bottom: 8 },
      });
      this.#observer = new ResizeObserver(() => {
        this.#editor?.layout();
        this.resized();
      });
      this.#observer.observe(host);
      this.#apply();
      this.#editor.layout();
      host.dataset["editorReady"] = "true";
    }).catch(() => {
      if (this.#generation === generation && this.#host === host) this.failed();
    });
  }

  #apply(): void {
    const api = this.#api;
    const editor = this.#editor;
    if (api === null || editor === null || this.#preview === this.#applied) return;
    editor.setModel(null);
    this.#model?.dispose();
    this.#model = null;
    this.#applied = this.#preview;
    if (this.#preview !== null) {
      const { path, content } = this.#preview;
      const uri = api.Uri.from({ scheme: "lethetic-view", authority: "launch", path: `/${path}` });
      this.#model = api.editor.createModel(content, fileLanguage(path, api.languages.getLanguages()), uri);
      editor.setModel(this.#model);
      editor.updateOptions({ ariaLabel: `Read-only file preview: ${path}` });
    }
  }

  dispose(): void {
    this.#generation += 1;
    this.#observer?.disconnect();
    this.#observer = null;
    this.#editor?.setModel(null);
    this.#model?.dispose();
    this.#editor?.dispose();
    this.#model = null;
    this.#editor = null;
    this.#host = null;
    this.#api = null;
    this.#preview = null;
    this.#applied = null;
  }
}

export type DiffLayoutOption = "side-by-side" | "inline";

/** Read-only Monaco diff of one file: `HEAD` on the left or top, working tree on the right or bottom. */
export class MonacoDiffViewer {
  #host: HTMLElement | null = null;
  #api: MonacoApi | null = null;
  #editor: Monaco.editor.IStandaloneDiffEditor | null = null;
  #models: Monaco.editor.ITextModel[] = [];
  #diff: GitDiffResponse | null = null;
  #applied: GitDiffResponse | null = null;
  #layout: DiffLayoutOption = "side-by-side";
  #observer: ResizeObserver | null = null;
  #generation = 0;

  constructor(readonly failed: () => void) {}

  sync(host: HTMLElement | null, diff: GitDiffResponse | null, layout: DiffLayoutOption): void {
    if (host === this.#host) {
      this.#diff = diff;
      this.#layout = layout;
      this.#apply();
      return;
    }
    this.dispose();
    if (host === null) return;
    this.#host = host;
    this.#diff = diff;
    this.#layout = layout;
    const generation = this.#generation;
    void loadMonaco().then((api) => {
      if (this.#generation !== generation || this.#host !== host || !host.isConnected) return;
      this.#api = api;
      this.#editor = api.editor.createDiffEditor(host, {
        readOnly: true, domReadOnly: true, originalEditable: false, theme: "vs-dark",
        automaticLayout: false, renderSideBySide: layout === "side-by-side",
        useInlineViewWhenSpaceIsLimited: false, enableSplitViewResizing: true,
        ignoreTrimWhitespace: false, renderIndicators: true, renderOverviewRuler: true,
        minimap: { enabled: false }, links: false, contextmenu: false,
        glyphMargin: false, lineNumbers: "on", wordWrap: "off",
        scrollBeyondLastLine: false, fontSize: 13, fontFamily: "monospace",
        quickSuggestions: false, occurrencesHighlight: "off", selectionHighlight: false,
        codeLens: false, stickyScroll: { enabled: false }, renderValidationDecorations: "off",
        unicodeHighlight: { ambiguousCharacters: false, invisibleCharacters: false },
        padding: { top: 8, bottom: 8 },
        ariaLabel: "Read-only git diff",
      });
      this.#observer = new ResizeObserver(() => this.#editor?.layout());
      this.#observer.observe(host);
      this.#apply();
      this.#editor.layout();
      host.dataset["editorReady"] = "true";
    }).catch(() => {
      if (this.#generation === generation && this.#host === host) this.failed();
    });
  }

  #apply(): void {
    const api = this.#api;
    const editor = this.#editor;
    if (api === null || editor === null) return;
    editor.updateOptions({ renderSideBySide: this.#layout === "side-by-side" });
    if (this.#diff === this.#applied) return;
    editor.setModel(null);
    for (const model of this.#models) model.dispose();
    this.#models = [];
    this.#applied = this.#diff;
    if (this.#diff === null) return;
    const { path, original, modified } = this.#diff;
    const language = fileLanguage(path, api.languages.getLanguages());
    const model = (side: string, content: string): Monaco.editor.ITextModel =>
      api.editor.createModel(content, language,
        api.Uri.from({ scheme: "lethetic-diff", authority: side, path: `/${path}` }));
    this.#models = [model("head", original), model("working", modified)];
    editor.setModel({ original: this.#models[0] as Monaco.editor.ITextModel, modified: this.#models[1] as Monaco.editor.ITextModel });
  }

  dispose(): void {
    this.#generation += 1;
    this.#observer?.disconnect();
    this.#observer = null;
    this.#editor?.setModel(null);
    for (const model of this.#models) model.dispose();
    this.#editor?.dispose();
    this.#models = [];
    this.#editor = null;
    this.#host = null;
    this.#api = null;
    this.#diff = null;
    this.#applied = null;
  }
}
