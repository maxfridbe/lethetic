import { FILE_ARCHIVE_BYTES, FILE_DOWNLOAD_BYTES, FILE_JSON_BYTES, FileServiceError, parseFileError, parseFileList, parseFileRead, validFilePath, } from "./protocol.js";
export async function boundedResponseBytes(response, maximum, signal) {
    const announced = response.headers.get("Content-Length");
    if (announced !== null && (!/^[0-9]+$/u.test(announced) || Number(announced) > maximum)) {
        await response.body?.cancel();
        throw new FileServiceError("too_large");
    }
    if (response.body === null)
        throw new FileServiceError("unavailable");
    const reader = response.body.getReader();
    const cancel = () => { void reader.cancel().catch(() => { }); };
    signal.addEventListener("abort", cancel, { once: true });
    let bytes = new Uint8Array(Math.min(maximum, announced === null ? 65536 : Number(announced)));
    let length = 0;
    try {
        for (;;) {
            if (signal.aborted)
                throw new FileServiceError("cancelled");
            const next = await reader.read();
            if (signal.aborted)
                throw new FileServiceError("cancelled");
            if (next.done)
                break;
            const required = length + next.value.byteLength;
            if (required > maximum)
                throw new FileServiceError("too_large");
            if (required > bytes.length) {
                const grown = new Uint8Array(Math.min(maximum, Math.max(required, bytes.length * 2, 65536)));
                grown.set(bytes);
                bytes = grown;
            }
            bytes.set(next.value, length);
            length = required;
        }
        if (announced !== null && Number(announced) !== length)
            throw new FileServiceError("unavailable");
        return bytes.subarray(0, length);
    }
    finally {
        signal.removeEventListener("abort", cancel);
        await reader.cancel().catch(() => { });
        reader.releaseLock();
    }
}
async function jsonBody(response, maximum, signal) {
    if (response.headers.get("Content-Type") !== "application/json") {
        await response.body?.cancel();
        throw new FileServiceError("unavailable");
    }
    const bytes = await boundedResponseBytes(response, maximum, signal);
    try {
        return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    }
    catch {
        throw new FileServiceError("unavailable");
    }
}
function archiveExclusions(headers) {
    const count = (category) => {
        const value = headers.get(`x-lethetic-files-excluded-${category}`);
        if (value === null || !/^(0|[1-9][0-9]{0,9})$/u.test(value) || Number(value) > 0xffffffff) {
            throw new FileServiceError("unavailable");
        }
        return Number(value);
    };
    return { protected: count("protected"), unsupported: count("unsupported"), unreadable: count("unreadable") };
}
export class FileClient {
    post;
    #active = 0;
    constructor(post) {
        this.post = post;
    }
    async list(path, signal) {
        return this.#request("/api/files/list", path, signal, async (response, signal) => {
            const value = parseFileList(await jsonBody(response, FILE_JSON_BYTES, signal), path);
            if (value === null)
                throw new FileServiceError("unavailable");
            return value;
        });
    }
    async read(path, signal) {
        return this.#request("/api/files/read", path, signal, async (response, signal) => {
            const value = parseFileRead(await jsonBody(response, FILE_JSON_BYTES, signal), path);
            if (value === null)
                throw new FileServiceError("unavailable");
            return value;
        });
    }
    async download(path, archive, signal) {
        return this.#request(archive ? "/api/files/archive" : "/api/files/download", path, signal, async (response, signal) => {
            const disposition = response.headers.get("Content-Disposition");
            const match = disposition?.match(/^attachment; filename="([A-Za-z0-9_-][A-Za-z0-9._-]{0,95})"$/u);
            const filename = match?.[1];
            const mime = response.headers.get("Content-Type");
            if (filename === undefined || mime === null ||
                !/^[a-z0-9-]+\/[a-z0-9.+-]+(?:; charset=utf-8)?$/u.test(mime) ||
                (archive && (mime !== "application/zip" || !filename.endsWith(".zip")))) {
                await response.body?.cancel();
                throw new FileServiceError("unavailable");
            }
            const exclusions = archive ? archiveExclusions(response.headers) : null;
            const bytes = await boundedResponseBytes(response, archive ? FILE_ARCHIVE_BYTES : FILE_DOWNLOAD_BYTES, signal);
            return { blob: new Blob([bytes], { type: mime }), filename, exclusions };
        });
    }
    async #request(endpoint, path, signal, consume) {
        if (!validFilePath(path, endpoint === "/api/files/list" || endpoint === "/api/files/archive")) {
            throw new FileServiceError("bad_request");
        }
        if (this.#active >= 2)
            throw new FileServiceError("busy");
        if (signal.aborted)
            throw new FileServiceError("cancelled");
        this.#active += 1;
        const abort = new AbortController();
        const cancel = () => abort.abort();
        signal.addEventListener("abort", cancel, { once: true });
        let timedOut = false;
        const deadline = globalThis.setTimeout(() => {
            timedOut = true;
            abort.abort();
        }, endpoint === "/api/files/archive" ? 55_000 : 25_000);
        try {
            const response = await this.post(endpoint, path, abort.signal);
            if (!response.ok)
                throw parseFileError(await jsonBody(response, 8192, abort.signal));
            const result = await consume(response, abort.signal);
            if (abort.signal.aborted)
                throw new FileServiceError("cancelled");
            return result;
        }
        catch (error) {
            if (timedOut)
                throw new FileServiceError("deadline");
            if (signal.aborted)
                throw new FileServiceError("cancelled");
            throw error instanceof FileServiceError ? error : new FileServiceError("unavailable");
        }
        finally {
            globalThis.clearTimeout(deadline);
            signal.removeEventListener("abort", cancel);
            this.#active -= 1;
        }
    }
}
