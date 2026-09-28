let assets = null;
async function loadMonaco() {
    if (assets !== null)
        return assets;
    assets = (async () => {
        const environment = globalThis;
        environment.MonacoEnvironment = {
            getWorker: (_module, label) => {
                if (label !== "editorWorkerService")
                    throw new Error("Unsupported editor worker");
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
        const styled = new Promise((resolve, reject) => {
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
export function fileLanguage(path, languages) {
    const name = path.split("/").at(-1) ?? "";
    return languages.find((language) => language.filenames?.includes(name))?.id ??
        languages.find((language) => language.extensions?.some((extension) => name.endsWith(extension)))?.id ??
        "plaintext";
}
export class MonacoFileEditor {
    failed;
    resized;
    #host = null;
    #api = null;
    #editor = null;
    #model = null;
    #preview = null;
    #applied = null;
    #observer = null;
    #generation = 0;
    constructor(failed, resized) {
        this.failed = failed;
        this.resized = resized;
    }
    sync(host, preview) {
        if (host === this.#host) {
            this.#preview = preview;
            this.#apply();
            return;
        }
        this.dispose();
        if (host === null)
            return;
        this.#host = host;
        this.#preview = preview;
        const generation = this.#generation;
        void loadMonaco().then((api) => {
            if (this.#generation !== generation || this.#host !== host || !host.isConnected)
                return;
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
            if (this.#generation === generation && this.#host === host)
                this.failed();
        });
    }
    #apply() {
        const api = this.#api;
        const editor = this.#editor;
        if (api === null || editor === null || this.#preview === this.#applied)
            return;
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
    dispose() {
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
/** Read-only Monaco diff of one file: `HEAD` on the left or top, working tree on the right or bottom. */
export class MonacoDiffViewer {
    failed;
    #host = null;
    #api = null;
    #editor = null;
    #models = [];
    #diff = null;
    #applied = null;
    #layout = "side-by-side";
    #observer = null;
    #generation = 0;
    constructor(failed) {
        this.failed = failed;
    }
    sync(host, diff, layout) {
        if (host === this.#host) {
            this.#diff = diff;
            this.#layout = layout;
            this.#apply();
            return;
        }
        this.dispose();
        if (host === null)
            return;
        this.#host = host;
        this.#diff = diff;
        this.#layout = layout;
        const generation = this.#generation;
        void loadMonaco().then((api) => {
            if (this.#generation !== generation || this.#host !== host || !host.isConnected)
                return;
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
            if (this.#generation === generation && this.#host === host)
                this.failed();
        });
    }
    #apply() {
        const api = this.#api;
        const editor = this.#editor;
        if (api === null || editor === null)
            return;
        editor.updateOptions({ renderSideBySide: this.#layout === "side-by-side" });
        if (this.#diff === this.#applied)
            return;
        editor.setModel(null);
        for (const model of this.#models)
            model.dispose();
        this.#models = [];
        this.#applied = this.#diff;
        if (this.#diff === null)
            return;
        const { path, original, modified } = this.#diff;
        const language = fileLanguage(path, api.languages.getLanguages());
        const model = (side, content) => api.editor.createModel(content, language, api.Uri.from({ scheme: "lethetic-diff", authority: side, path: `/${path}` }));
        this.#models = [model("head", original), model("working", modified)];
        editor.setModel({ original: this.#models[0], modified: this.#models[1] });
    }
    dispose() {
        this.#generation += 1;
        this.#observer?.disconnect();
        this.#observer = null;
        this.#editor?.setModel(null);
        for (const model of this.#models)
            model.dispose();
        this.#editor?.dispose();
        this.#models = [];
        this.#editor = null;
        this.#host = null;
        this.#api = null;
        this.#diff = null;
        this.#applied = null;
    }
}
