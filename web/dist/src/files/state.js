import { exclusionLabel, fileFailure } from "./protocol.js";
function initialState(diffLayout = "side-by-side") {
    return { open: false, tab: "files", directory: "", listing: null, listingBusy: false, listingError: null,
        selectedPath: null, preview: null, previewBusy: false, previewError: null, downloadBusy: false,
        changes: null, changesBusy: false, changesError: null, collapsed: new Set(),
        selectedChange: null, diff: null, diffBusy: false, diffError: null, diffLayout };
}
export class FilesController {
    changed;
    notify;
    #state = initialState();
    #client = null;
    #list = null;
    #read = null;
    #download = null;
    #status = null;
    #diff = null;
    #epoch = 0;
    #urls = new Map();
    constructor(changed, notify) {
        this.changed = changed;
        this.notify = notify;
    }
    get state() { return this.#state; }
    attach(client) { this.#client = client; }
    reset() {
        this.#epoch += 1;
        this.#abort();
        this.#state = initialState(this.#state.diffLayout);
        for (const [url, timer] of this.#urls) {
            globalThis.clearTimeout(timer);
            URL.revokeObjectURL(url);
        }
        this.#urls.clear();
    }
    suspend() {
        this.#abort();
        this.#state = { ...this.#state, listingBusy: false, previewBusy: false, downloadBusy: false,
            changesBusy: false, diffBusy: false };
    }
    toggle() {
        if (this.#state.open) {
            this.reset();
            this.changed();
            return;
        }
        this.#state = { ...this.#state, open: true };
        this.changed();
        void this.navigate("");
    }
    showTab(tab) {
        if (!this.#state.open || this.#state.tab === tab)
            return;
        this.#state = { ...this.#state, tab };
        this.changed();
        if (tab === "changes" && this.#state.changes === null && !this.#state.changesBusy) {
            void this.refreshChanges();
        }
    }
    setDiffLayout(diffLayout) {
        if (this.#state.diffLayout === diffLayout)
            return;
        this.#state = { ...this.#state, diffLayout };
        this.changed();
    }
    toggleFolder(path) {
        const collapsed = new Set(this.#state.collapsed);
        if (!collapsed.delete(path))
            collapsed.add(path);
        this.#state = { ...this.#state, collapsed };
        this.changed();
    }
    async refreshChanges() {
        const client = this.#client;
        if (client === null || !this.#state.open)
            return;
        this.#status?.abort();
        const request = new AbortController();
        this.#status = request;
        const selected = this.#state.selectedChange;
        this.#state = { ...this.#state, changesBusy: true, changesError: null };
        this.changed();
        try {
            const changes = await client.gitStatus(request.signal);
            if (this.#status !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, changes, changesBusy: false };
            const still = selected !== null && changes.files.some((file) => file.path === selected);
            if (still) {
                void this.selectChange(selected);
            }
            else {
                this.#diff?.abort();
                this.#diff = null;
                this.#state = { ...this.#state, selectedChange: null, diff: null, diffBusy: false, diffError: null };
            }
        }
        catch (error) {
            if (this.#status !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, changesBusy: false, changesError: fileFailure(error) };
        }
        finally {
            if (this.#status === request) {
                this.#status = null;
                this.changed();
            }
        }
    }
    async selectChange(path) {
        const client = this.#client;
        if (client === null || !this.#state.open)
            return;
        this.#diff?.abort();
        const request = new AbortController();
        this.#diff = request;
        this.#state = { ...this.#state, selectedChange: path, diff: null, diffBusy: true, diffError: null };
        this.changed();
        try {
            const diff = await client.gitDiff(path, request.signal);
            if (this.#diff !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, diff, diffBusy: false };
        }
        catch (error) {
            if (this.#diff !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, diffBusy: false, diffError: fileFailure(error) };
        }
        finally {
            if (this.#diff === request) {
                this.#diff = null;
                this.changed();
            }
        }
    }
    async navigate(path) {
        const client = this.#client;
        if (client === null || !this.#state.open)
            return null;
        this.#list?.abort();
        this.#read?.abort();
        this.#read = null;
        const request = new AbortController();
        this.#list = request;
        this.#state = { ...this.#state, directory: path, listing: null, listingBusy: true,
            listingError: null, selectedPath: null, preview: null, previewBusy: false, previewError: null };
        this.changed();
        try {
            const listing = await client.list(path, request.signal);
            if (this.#list !== request || request.signal.aborted)
                return null;
            this.#state = { ...this.#state, listing, listingBusy: false };
            return listing;
        }
        catch (error) {
            if (this.#list !== request || request.signal.aborted)
                return null;
            this.#state = { ...this.#state, listingBusy: false, listingError: fileFailure(error) };
            return null;
        }
        finally {
            if (this.#list === request) {
                this.#list = null;
                this.changed();
            }
        }
    }
    async select(path) {
        const client = this.#client;
        if (client === null || !this.#state.open)
            return;
        this.#read?.abort();
        const request = new AbortController();
        this.#read = request;
        this.#state = { ...this.#state, selectedPath: path, preview: null, previewBusy: true, previewError: null };
        this.changed();
        try {
            const preview = await client.read(path, request.signal);
            if (this.#read !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, preview, previewBusy: false };
        }
        catch (error) {
            if (this.#read !== request || request.signal.aborted)
                return;
            this.#state = { ...this.#state, previewBusy: false, previewError: fileFailure(error) };
        }
        finally {
            if (this.#read === request) {
                this.#read = null;
                this.changed();
            }
        }
    }
    async refresh() {
        const path = this.#state.selectedPath;
        const listing = await this.navigate(this.#state.directory);
        if (path !== null && listing !== null && this.#state.open &&
            this.#state.listing === listing && this.#state.selectedPath === null &&
            listing.entries.some((entry) => entry.path === path)) {
            await this.select(path);
        }
    }
    async download(archive) {
        const client = this.#client;
        const path = archive ? this.#state.directory : this.#state.selectedPath;
        if (client === null || path === null || this.#download !== null || !this.#state.open)
            return;
        const request = new AbortController();
        this.#download = request;
        this.#state = { ...this.#state, downloadBusy: true };
        this.changed();
        try {
            const result = await client.download(path, archive, request.signal);
            if (this.#download !== request || request.signal.aborted)
                return;
            const url = URL.createObjectURL(result.blob);
            const anchor = globalThis.document.createElement("a");
            anchor.href = url;
            anchor.download = result.filename;
            anchor.rel = "noopener";
            anchor.hidden = true;
            globalThis.document.body.append(anchor);
            try {
                anchor.click();
            }
            finally {
                anchor.remove();
            }
            const timer = globalThis.setTimeout(() => {
                URL.revokeObjectURL(url);
                this.#urls.delete(url);
            }, 60_000);
            this.#urls.set(url, timer);
            const exclusions = result.exclusions === null ? null : exclusionLabel(result.exclusions);
            this.notify(exclusions === null ? "Download ready." : `Download ready. ${exclusions}`);
        }
        catch (error) {
            if (this.#download === request && !request.signal.aborted)
                this.notify(fileFailure(error));
        }
        finally {
            if (this.#download === request) {
                this.#download = null;
                this.#state = { ...this.#state, downloadBusy: false };
                this.changed();
            }
        }
    }
    async copyPath() {
        const path = this.#state.selectedPath ?? this.#state.directory;
        if (path === "" || !this.#state.open)
            return;
        const epoch = this.#epoch;
        try {
            await globalThis.navigator.clipboard.writeText(path);
            if (epoch === this.#epoch)
                this.notify("Copied the launch-root-relative path. The prompt was not changed.");
        }
        catch {
            if (epoch === this.#epoch)
                this.notify("Clipboard unavailable. Select and copy the displayed relative path instead.");
        }
    }
    #abort() {
        this.#list?.abort();
        this.#read?.abort();
        this.#download?.abort();
        this.#status?.abort();
        this.#diff?.abort();
        this.#list = this.#read = this.#download = this.#status = this.#diff = null;
    }
}
