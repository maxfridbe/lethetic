import { WFE_MAX_COMMAND_MESSAGE_BYTES } from "./generated/contracts.js";
import { clientContractGate, commandResponseMatchesRequest, parseServerMessage, } from "./protocol.js";
import { assertNever, newRequestId } from "./safety.js";
import { FileServiceError, endpointAllowsRoot, validFilePath } from "./files/protocol.js";
const MAX_IN_FLIGHT = 64;
const MAX_OUTGOING_MESSAGE_BYTES = WFE_MAX_COMMAND_MESSAGE_BYTES;
const MAX_SOCKET_BUFFERED_BYTES = 1024 * 1024;
const COMPLETED_REPLAY_WINDOW = 512;
const BASE_RECONNECT_DELAY_MS = 400;
const MAX_RECONNECT_DELAY_MS = 15_000;
const DEFAULT_AUTH_RETRY_DELAY_MS = 60_000;
const MAX_AUTH_RETRY_DELAY_MS = 5 * 60_000;
const SESSION_REAUTHENTICATION_PREOPEN_FAILURE_INTERVAL = 5;
const AUTHENTICATION_TIMEOUT_MS = 10_000;
const SYNCHRONIZATION_TIMEOUT_MS = 10_000;
const STALE_RECOVERY_TIMEOUT_MS = 10_000;
const LIVENESS_IDLE_MS = 20_000;
const LIVENESS_RESPONSE_TIMEOUT_MS = 10_000;
const WEBSOCKET_PROTOCOL_HEADER = "X-Lethetic-WebSocket-Protocol";
const WEBSOCKET_PROTOCOL_PATTERN = /^lethetic-wfe-v1\.[A-Za-z0-9_-]{43}$/u;
const UTF8_ENCODER = new TextEncoder();
function authenticationFailureLabel(status, mode) {
    switch (status) {
        case 401:
            return mode === "token"
                ? "Controller token rejected; reopen the URL printed by the current Lethetic process"
                : "Controller token authentication is required; reopen the private URL printed by Lethetic";
        case 403:
            return "Browser origin rejected; reopen the URL printed by the current Lethetic process";
        case 421:
            return "Request address rejected; open the exact printed URL";
        case 429:
            return "Too many authentication attempts; waiting before retrying";
        default:
            return `Authentication was rejected (HTTP ${String(status)})`;
    }
}
function retryAfterMilliseconds(response) {
    if (response.status !== 429) {
        return null;
    }
    const value = response.headers.get("Retry-After");
    if (value === null || !/^\d+$/u.test(value)) {
        return DEFAULT_AUTH_RETRY_DELAY_MS;
    }
    const seconds = Number(value);
    if (!Number.isSafeInteger(seconds) || seconds < 0) {
        return DEFAULT_AUTH_RETRY_DELAY_MS;
    }
    return Math.min(MAX_AUTH_RETRY_DELAY_MS, seconds * 1_000);
}
export class BrowserTransport {
    #events;
    #pending = new Map();
    #completed = new Map();
    #completedOrder = [];
    #socket = null;
    #reconnectTimer = null;
    #synchronizationTimer = null;
    #staleRecoveryTimer = null;
    #livenessTimer = null;
    #livenessResponseTimer = null;
    #websocketProtocol = null;
    #authenticationMode = null;
    #sessionAuthentication = null;
    #tokenAuthentication = null;
    #authenticationAbortController = null;
    #authenticationGeneration = 0;
    #pendingToken = null;
    #reconnectAction = "connect";
    #sessionProofHasOpened = false;
    #sessionProofPreopenFailures = 0;
    #lastRevision = 0;
    #generation = 0;
    #reconnectAttempt = 0;
    #permanentlyClosed = false;
    #suspendedForBfcache = false;
    #synchronized = false;
    #helloReceived = false;
    #filesEnabled = false;
    #snapshotRequestId = null;
    #livenessRequestId = null;
    #lastSendFailure = null;
    constructor(events) {
        this.#events = events;
        globalThis.addEventListener("online", this.#handleOnline);
        globalThis.addEventListener("offline", this.#handleOffline);
        globalThis.addEventListener("pagehide", this.#handlePageHide);
        globalThis.addEventListener("pageshow", this.#handlePageShow);
    }
    get pendingCount() {
        return this.#pending.size;
    }
    get lastSendFailure() {
        return this.#lastSendFailure;
    }
    authenticate(token) {
        this.#authenticationMode = "token";
        this.#websocketProtocol = null;
        this.#pendingToken = token;
        this.#sessionProofHasOpened = false;
        this.#sessionProofPreopenFailures = 0;
        return this.#startTokenAuthentication(false);
    }
    authenticateSession() {
        this.#authenticationMode = "session";
        this.#websocketProtocol = null;
        this.#pendingToken = null;
        this.#sessionProofHasOpened = false;
        this.#sessionProofPreopenFailures = 0;
        return this.#startSessionAuthentication(false);
    }
    #startTokenAuthentication(isReconnect) {
        if (this.#tokenAuthentication !== null) {
            return this.#tokenAuthentication;
        }
        const task = this.#runTokenAuthentication(isReconnect);
        this.#tokenAuthentication = task;
        const clear = () => {
            if (this.#tokenAuthentication === task) {
                this.#tokenAuthentication = null;
            }
        };
        void task.then(clear, clear);
        return task;
    }
    async #runTokenAuthentication(isReconnect) {
        const token = this.#pendingToken;
        if (token === null) {
            return;
        }
        const attempt = await this.#requestAuthentication("/auth", JSON.stringify({ token }), "token");
        if (attempt.cancelled || this.#pendingToken !== token) {
            return;
        }
        if (attempt.websocketProtocol === null) {
            if (attempt.retryable) {
                this.#scheduleReconnect("token_auth", attempt.retryAfterMilliseconds ?? 0);
            }
            else {
                this.#pendingToken = null;
            }
            return;
        }
        this.#pendingToken = null;
        this.#websocketProtocol = attempt.websocketProtocol;
        this.#connect(isReconnect);
    }
    #startSessionAuthentication(isReconnect) {
        if (this.#sessionAuthentication !== null) {
            return this.#sessionAuthentication;
        }
        const task = this.#runSessionAuthentication(isReconnect);
        this.#sessionAuthentication = task;
        const clear = () => {
            if (this.#sessionAuthentication === task) {
                this.#sessionAuthentication = null;
            }
        };
        void task.then(clear, clear);
        return task;
    }
    async #runSessionAuthentication(isReconnect) {
        const attempt = await this.#requestAuthentication("/auth/session", JSON.stringify({}), "session");
        if (attempt.cancelled) {
            return;
        }
        if (attempt.websocketProtocol === null) {
            if (attempt.retryable) {
                this.#scheduleReconnect("session_auth", attempt.retryAfterMilliseconds ?? 0);
            }
            return;
        }
        const previousProtocol = this.#websocketProtocol;
        const replacesAuthentication = previousProtocol !== null &&
            previousProtocol !== attempt.websocketProtocol;
        if (replacesAuthentication) {
            this.#abandonPendingForNewAuthenticationEpoch();
        }
        this.#sessionProofHasOpened = false;
        this.#sessionProofPreopenFailures = 0;
        this.#websocketProtocol = attempt.websocketProtocol;
        this.#connect(isReconnect || previousProtocol !== null);
    }
    async #requestAuthentication(endpoint, body, mode) {
        const failed = (retryable = false, retryAfter = null) => ({
            websocketProtocol: null,
            retryable,
            retryAfterMilliseconds: retryAfter,
            cancelled: false,
        });
        const cancelled = () => ({
            websocketProtocol: null,
            retryable: false,
            retryAfterMilliseconds: null,
            cancelled: true,
        });
        if (this.#permanentlyClosed || this.#suspendedForBfcache) {
            return cancelled();
        }
        if (!clientContractGate()) {
            this.closePermanently("The browser application schema is unsupported.");
            return cancelled();
        }
        if (globalThis.location.protocol !== "https:") {
            this.closePermanently("A secure HTTPS origin is required.");
            return cancelled();
        }
        this.#clearReconnectTimer();
        this.#emitStatus("authenticating", mode === "token" ? "Authenticating" : "Establishing controller session", this.#reconnectAttempt, null);
        const authenticationGeneration = this.#authenticationGeneration + 1;
        this.#authenticationGeneration = authenticationGeneration;
        this.#authenticationAbortController?.abort();
        const abortController = new AbortController();
        this.#authenticationAbortController = abortController;
        let timedOut = false;
        const timeout = globalThis.setTimeout(() => {
            timedOut = true;
            abortController.abort();
        }, AUTHENTICATION_TIMEOUT_MS);
        try {
            let response;
            try {
                response = await globalThis.fetch(endpoint, {
                    method: "POST",
                    credentials: "same-origin",
                    mode: "cors",
                    cache: "no-store",
                    redirect: "error",
                    referrerPolicy: "no-referrer",
                    headers: { "Content-Type": "application/json" },
                    body,
                    signal: abortController.signal,
                });
            }
            catch {
                if (authenticationGeneration !== this.#authenticationGeneration ||
                    this.#permanentlyClosed ||
                    this.#suspendedForBfcache) {
                    return cancelled();
                }
                this.#emitStatus("auth_failed", timedOut
                    ? "Authentication timed out"
                    : "Authentication could not reach the server", this.#reconnectAttempt, null);
                return failed(true);
            }
            if (authenticationGeneration !== this.#authenticationGeneration ||
                this.#permanentlyClosed ||
                this.#suspendedForBfcache) {
                return cancelled();
            }
            if (!response.ok) {
                const retryAfter = retryAfterMilliseconds(response);
                this.#emitStatus("auth_failed", authenticationFailureLabel(response.status, mode), this.#reconnectAttempt, retryAfter);
                return failed(response.status === 429 || response.status >= 500, retryAfter);
            }
            const websocketProtocol = response.headers.get(WEBSOCKET_PROTOCOL_HEADER);
            if (websocketProtocol === null ||
                !WEBSOCKET_PROTOCOL_PATTERN.test(websocketProtocol)) {
                this.#emitStatus("auth_failed", "Authentication returned an invalid connection proof", this.#reconnectAttempt, null);
                return failed();
            }
            return {
                websocketProtocol,
                retryable: false,
                retryAfterMilliseconds: null,
                cancelled: false,
            };
        }
        finally {
            globalThis.clearTimeout(timeout);
            if (this.#authenticationAbortController === abortController) {
                this.#authenticationAbortController = null;
            }
        }
    }
    #cancelAuthenticationRequest() {
        this.#authenticationGeneration += 1;
        const abortController = this.#authenticationAbortController;
        this.#authenticationAbortController = null;
        this.#sessionAuthentication = null;
        this.#tokenAuthentication = null;
        abortController?.abort();
    }
    send(command, expectedRevision) {
        this.#lastSendFailure = null;
        if (!this.#canSendUserRequest()) {
            this.#lastSendFailure = "The command connection is not synchronized.";
            return null;
        }
        if (this.#pending.size >= MAX_IN_FLIGHT) {
            this.#lastSendFailure = "Too many commands are awaiting responses.";
            return null;
        }
        const request = Object.assign({ id: this.#uniqueId(), expected_revision: expectedRevision }, command);
        if (!this.#queueAndSend(request)) {
            return null;
        }
        return request;
    }
    requestSnapshot(expectedRevision) {
        if (this.#snapshotRequestId !== null ||
            !this.#helloReceived ||
            !this.#socketIsOpen() ||
            this.#pending.size >= MAX_IN_FLIGHT) {
            return null;
        }
        const request = {
            id: this.#uniqueId(),
            expected_revision: expectedRevision,
            type: "request_snapshot",
        };
        if (!this.#queueAndSend(request, true)) {
            return null;
        }
        this.#snapshotRequestId = request.id;
        return request;
    }
    noteHello(readOnlyFiles = false) {
        this.#helloReceived = true;
        this.#filesEnabled = readOnlyFiles;
    }
    async requestFile(endpoint, path, signal) {
        const proof = this.#websocketProtocol;
        const generation = this.#authenticationGeneration;
        if (!this.#filesEnabled || !this.#canSendUserRequest() || proof === null ||
            !WEBSOCKET_PROTOCOL_PATTERN.test(proof) || globalThis.location.protocol !== "https:") {
            throw new FileServiceError("unavailable");
        }
        if (!["/api/files/list", "/api/files/read", "/api/files/download", "/api/files/archive",
            "/api/git/status", "/api/git/diff"].includes(endpoint) ||
            !validFilePath(path, endpointAllowsRoot(endpoint))) {
            throw new FileServiceError("bad_request");
        }
        const body = JSON.stringify({ path });
        if (UTF8_ENCODER.encode(body).byteLength > 8192)
            throw new FileServiceError("bad_request");
        const response = await globalThis.fetch(endpoint, {
            method: "POST", credentials: "same-origin", mode: "cors", cache: "no-store",
            redirect: "error", referrerPolicy: "no-referrer",
            headers: { "Content-Type": "application/json", [WEBSOCKET_PROTOCOL_HEADER]: proof },
            body, signal,
        });
        if (signal.aborted || proof !== this.#websocketProtocol ||
            generation !== this.#authenticationGeneration || this.#permanentlyClosed || this.#suspendedForBfcache) {
            await response.body?.cancel();
            throw new FileServiceError("cancelled");
        }
        return response;
    }
    markSynchronized() {
        if (this.#permanentlyClosed || !this.#socketIsOpen()) {
            return;
        }
        this.#synchronized = true;
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#reconnectAttempt = 0;
        this.#reconnectAction = "connect";
        this.#emitStatus("live", "Connected", 0, null);
        this.#replayPending();
        if (this.#livenessRequestId === null) {
            this.#armLivenessTimer();
        }
        else {
            this.#armLivenessResponseTimer();
        }
    }
    markStale(label = "Resynchronizing") {
        this.#synchronized = false;
        this.#clearLivenessTimer();
        if (this.#socketIsOpen()) {
            this.#emitStatus("synchronizing", label, this.#reconnectAttempt, null);
            this.#armStaleRecoveryTimer();
        }
    }
    closePermanently(reason) {
        this.#permanentlyClosed = true;
        this.#suspendedForBfcache = false;
        this.#synchronized = false;
        this.#cancelAuthenticationRequest();
        this.#clearReconnectTimer();
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#clearLivenessTimer();
        this.#clearLivenessResponseTimer();
        this.#livenessRequestId = null;
        this.#websocketProtocol = null;
        this.#pendingToken = null;
        this.#reconnectAction = "connect";
        this.#sessionProofHasOpened = false;
        this.#sessionProofPreopenFailures = 0;
        const socket = this.#socket;
        this.#socket = null;
        if (socket !== null && socket.readyState < WebSocket.CLOSING) {
            socket.close(1000, "client closed");
        }
        this.#emitStatus("incompatible", reason, this.#reconnectAttempt, null);
    }
    #queueAndSend(request, allowUnsynchronized = false, internal = false) {
        const encoded = JSON.stringify(request);
        if (UTF8_ENCODER.encode(encoded).byteLength > MAX_OUTGOING_MESSAGE_BYTES) {
            this.#lastSendFailure =
                "The encoded command exceeds the 256 KiB WebSocket message limit.";
            return false;
        }
        const pending = {
            request,
            encoded,
            internal,
            sentGeneration: 0,
        };
        this.#pending.set(request.id, pending);
        this.#sendPending(pending, allowUnsynchronized);
        return true;
    }
    #sendPending(pending, allowUnsynchronized) {
        const socket = this.#socket;
        if (socket === null ||
            socket.readyState !== WebSocket.OPEN ||
            (!allowUnsynchronized && !this.#synchronized)) {
            return;
        }
        if (socket.bufferedAmount > MAX_SOCKET_BUFFERED_BYTES) {
            socket.close(1008, "client output queue full");
            return;
        }
        try {
            socket.send(pending.encoded);
            pending.sentGeneration = this.#generation;
        }
        catch {
            socket.close(1011, "command send failed");
        }
    }
    #abandonPendingForNewAuthenticationEpoch() {
        const abandoned = Array.from(this.#pending.values())
            .filter((pending) => !pending.internal)
            .map((pending) => pending.request);
        this.#pending.clear();
        this.#completed.clear();
        this.#completedOrder.length = 0;
        this.#snapshotRequestId = null;
        this.#livenessRequestId = null;
        this.#lastRevision = 0;
        this.#synchronized = false;
        this.#helloReceived = false;
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#clearLivenessTimer();
        this.#clearLivenessResponseTimer();
        this.#events.authenticationEpochChanged(abandoned);
    }
    #replayPending() {
        for (const pending of this.#pending.values()) {
            if (pending.sentGeneration !== this.#generation) {
                this.#sendPending(pending, false);
            }
        }
    }
    #uniqueId() {
        let id = newRequestId();
        while (this.#pending.has(id) || this.#completed.has(id)) {
            id = newRequestId();
        }
        return id;
    }
    #canSendUserRequest() {
        return (!this.#permanentlyClosed &&
            this.#synchronized &&
            this.#socketIsOpen());
    }
    #socketIsOpen() {
        return this.#socket?.readyState === WebSocket.OPEN;
    }
    #connect(isReconnect) {
        if (this.#permanentlyClosed ||
            this.#suspendedForBfcache ||
            !globalThis.navigator.onLine) {
            if (!globalThis.navigator.onLine) {
                this.#emitStatus("offline", "Offline; waiting for network", this.#reconnectAttempt, null);
            }
            return;
        }
        const websocketProtocol = this.#websocketProtocol;
        if (websocketProtocol === null ||
            !WEBSOCKET_PROTOCOL_PATTERN.test(websocketProtocol)) {
            this.#emitStatus("auth_failed", "A fresh controller bootstrap is required", this.#reconnectAttempt, null);
            return;
        }
        const previousSocket = this.#socket;
        this.#socket = null;
        if (previousSocket !== null &&
            previousSocket.readyState < WebSocket.CLOSING) {
            previousSocket.close(1000, "superseded");
        }
        this.#clearReconnectTimer();
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#clearLivenessTimer();
        this.#clearLivenessResponseTimer();
        this.#synchronized = false;
        this.#helloReceived = false;
        this.#generation += 1;
        this.#emitStatus(isReconnect ? "reconnecting" : "connecting", isReconnect ? "Reconnecting" : "Connecting", this.#reconnectAttempt, null);
        const endpoint = new URL("/ws", globalThis.location.origin);
        endpoint.protocol = "wss:";
        let socket;
        try {
            socket = new WebSocket(endpoint, websocketProtocol);
        }
        catch {
            this.#scheduleReconnect("connect");
            return;
        }
        let opened = false;
        this.#socket = socket;
        socket.addEventListener("open", () => {
            if (this.#socket !== socket ||
                this.#permanentlyClosed ||
                this.#suspendedForBfcache) {
                socket.close(1000, "superseded");
                return;
            }
            opened = true;
            if (socket.protocol !== websocketProtocol) {
                this.#events.protocolFault("The server did not echo the authenticated WebSocket proof.");
                socket.close(1002, "connection proof mismatch");
                return;
            }
            if (this.#authenticationMode === "session") {
                this.#sessionProofHasOpened = true;
                this.#sessionProofPreopenFailures = 0;
            }
            this.#emitStatus("synchronizing", "Waiting for server state", this.#reconnectAttempt, null);
            this.#events.socketOpened();
            this.#clearSynchronizationTimer();
            this.#synchronizationTimer = globalThis.setTimeout(() => {
                this.#synchronizationTimer = null;
                if (this.#socket === socket && !this.#synchronized) {
                    this.#events.protocolFault("The server did not complete hello and snapshot synchronization in time.");
                    socket.close(1002, "synchronization timeout");
                }
            }, SYNCHRONIZATION_TIMEOUT_MS);
        });
        socket.addEventListener("message", (event) => {
            if (this.#socket !== socket ||
                this.#permanentlyClosed ||
                this.#suspendedForBfcache) {
                return;
            }
            const parsed = parseServerMessage(event.data);
            if (!parsed.ok) {
                this.#events.protocolFault(parsed.reason);
                socket.close(1002, "protocol error");
                return;
            }
            this.#noteServerActivity(parsed.message);
            this.#routeMessage(parsed.message);
        });
        socket.addEventListener("close", () => {
            if (this.#socket !== socket) {
                return;
            }
            this.#socket = null;
            this.#clearSynchronizationTimer();
            this.#clearStaleRecoveryTimer();
            this.#clearLivenessTimer();
            this.#clearLivenessResponseTimer();
            this.#synchronized = false;
            this.#helloReceived = false;
            if (!this.#permanentlyClosed && !this.#suspendedForBfcache) {
                this.#scheduleReconnect(opened ? "connect" : this.#preopenReconnectAction());
            }
        });
        socket.addEventListener("error", () => {
            // The close event supplies the only actionable browser-safe signal.
        });
    }
    #routeMessage(message) {
        switch (message.type) {
            case "hello":
            case "state_snapshot":
            case "state_patch":
                this.#events.serverMessage(message);
                return;
            case "command_response":
                this.#routeResponse(message.response);
                return;
            default:
                assertNever(message, "transport server message");
        }
    }
    #routeResponse(response) {
        const pending = this.#pending.get(response.id);
        const encoded = JSON.stringify(response);
        if (pending === undefined) {
            const completed = this.#completed.get(response.id);
            if (completed === encoded) {
                return;
            }
            this.#events.protocolFault(completed === undefined
                ? "The server returned an unknown request ID."
                : "A replayed response conflicted with its original response.");
            this.#socket?.close(1002, "request correlation error");
            return;
        }
        if (!commandResponseMatchesRequest(response, pending.request)) {
            this.#events.protocolFault("The server response did not match its originating command.");
            this.#socket?.close(1002, "response identity mismatch");
            return;
        }
        const livenessResponse = this.#livenessRequestId === response.id;
        if (livenessResponse) {
            this.#clearLivenessResponseTimer();
            this.#livenessRequestId = null;
        }
        this.#pending.delete(response.id);
        if (this.#snapshotRequestId === response.id) {
            this.#snapshotRequestId = null;
        }
        if (livenessResponse && this.#synchronized) {
            this.#armLivenessTimer();
        }
        this.#rememberCompleted({ id: response.id, encoded });
        if (!pending.internal) {
            this.#events.commandResponse(response, pending.request);
        }
    }
    #rememberCompleted(response) {
        this.#completed.set(response.id, response.encoded);
        this.#completedOrder.push(response.id);
        while (this.#completedOrder.length > COMPLETED_REPLAY_WINDOW) {
            const oldest = this.#completedOrder.shift();
            if (oldest !== undefined) {
                this.#completed.delete(oldest);
            }
        }
    }
    #preopenReconnectAction() {
        if (this.#authenticationMode !== "session") {
            return "connect";
        }
        this.#sessionProofPreopenFailures += 1;
        if (this.#sessionProofHasOpened ||
            this.#sessionProofPreopenFailures >=
                SESSION_REAUTHENTICATION_PREOPEN_FAILURE_INTERVAL) {
            return "session_auth";
        }
        return "connect";
    }
    #scheduleReconnect(action = "connect", minimumDelayMilliseconds = 0) {
        if (action !== "connect") {
            this.#reconnectAction = action;
        }
        if (this.#permanentlyClosed ||
            this.#suspendedForBfcache ||
            this.#reconnectTimer !== null) {
            return;
        }
        const exponent = Math.min(this.#reconnectAttempt, 8);
        const backoff = Math.min(MAX_RECONNECT_DELAY_MS, BASE_RECONNECT_DELAY_MS * 2 ** exponent);
        const minimumDelay = Math.min(MAX_AUTH_RETRY_DELAY_MS, Math.max(0, minimumDelayMilliseconds));
        const delay = Math.max(backoff, minimumDelay);
        this.#reconnectAttempt += 1;
        if (globalThis.navigator.onLine) {
            this.#emitStatus("reconnecting", this.#reconnectLabel(), this.#reconnectAttempt, delay);
        }
        else {
            this.#emitStatus("offline", "Offline; retry remains scheduled", this.#reconnectAttempt, delay);
        }
        this.#reconnectTimer = globalThis.setTimeout(() => {
            this.#reconnectTimer = null;
            if (this.#permanentlyClosed ||
                this.#suspendedForBfcache ||
                !globalThis.navigator.onLine) {
                if (!globalThis.navigator.onLine) {
                    this.#emitStatus("offline", "Offline; waiting for network", this.#reconnectAttempt, null);
                }
                return;
            }
            this.#runReconnectAction();
        }, delay);
    }
    #reconnectLabel() {
        switch (this.#reconnectAction) {
            case "connect":
                return "Connection interrupted; retrying";
            case "session_auth":
                return "Controller session expired; reauthenticating";
            case "token_auth":
                return "Controller authentication interrupted; retrying";
            default:
                return assertNever(this.#reconnectAction, "reconnect action");
        }
    }
    #runReconnectAction() {
        const action = this.#reconnectAction;
        this.#reconnectAction = "connect";
        switch (action) {
            case "connect":
                this.#connect(true);
                return;
            case "session_auth":
                if (this.#authenticationMode === "session") {
                    void this.#startSessionAuthentication(true);
                }
                return;
            case "token_auth":
                if (this.#authenticationMode === "token") {
                    void this.#startTokenAuthentication(true);
                }
                return;
            default:
                assertNever(action, "reconnect action");
        }
    }
    #clearReconnectTimer() {
        if (this.#reconnectTimer !== null) {
            globalThis.clearTimeout(this.#reconnectTimer);
            this.#reconnectTimer = null;
        }
    }
    #clearSynchronizationTimer() {
        if (this.#synchronizationTimer !== null) {
            globalThis.clearTimeout(this.#synchronizationTimer);
            this.#synchronizationTimer = null;
        }
    }
    #armStaleRecoveryTimer() {
        if (this.#staleRecoveryTimer !== null) {
            return;
        }
        const socket = this.#socket;
        if (socket === null ||
            socket.readyState !== WebSocket.OPEN ||
            this.#permanentlyClosed ||
            this.#suspendedForBfcache) {
            return;
        }
        this.#staleRecoveryTimer = globalThis.setTimeout(() => {
            this.#staleRecoveryTimer = null;
            if (this.#socket === socket && !this.#synchronized) {
                socket.close(1011, "state recovery timeout");
            }
        }, STALE_RECOVERY_TIMEOUT_MS);
    }
    #clearStaleRecoveryTimer() {
        if (this.#staleRecoveryTimer !== null) {
            globalThis.clearTimeout(this.#staleRecoveryTimer);
            this.#staleRecoveryTimer = null;
        }
    }
    #noteServerActivity(message) {
        switch (message.type) {
            case "hello":
                this.#lastRevision = message.hello.revision;
                break;
            case "state_snapshot":
                this.#lastRevision = message.snapshot.revision;
                break;
            case "state_patch":
                this.#lastRevision = message.patch.revision;
                break;
            case "command_response":
                if (message.response.result.status === "ok") {
                    this.#lastRevision = Math.max(this.#lastRevision, message.response.result.revision);
                }
                else if (message.response.result.error.current_revision !== null) {
                    this.#lastRevision = Math.max(this.#lastRevision, message.response.result.error.current_revision);
                }
                break;
            default:
                assertNever(message, "liveness server message");
        }
        if (this.#synchronized && this.#livenessRequestId === null) {
            this.#armLivenessTimer();
        }
    }
    #armLivenessTimer() {
        this.#clearLivenessTimer();
        const socket = this.#socket;
        if (socket === null ||
            socket.readyState !== WebSocket.OPEN ||
            !this.#synchronized ||
            this.#permanentlyClosed ||
            this.#suspendedForBfcache) {
            return;
        }
        this.#livenessTimer = globalThis.setTimeout(() => {
            this.#livenessTimer = null;
            if (this.#socket !== socket ||
                socket.readyState !== WebSocket.OPEN ||
                !this.#synchronized) {
                return;
            }
            if (this.#snapshotRequestId !== null ||
                this.#pending.size >= MAX_IN_FLIGHT) {
                socket.close(1011, "liveness check blocked");
                return;
            }
            const request = {
                id: this.#uniqueId(),
                expected_revision: this.#lastRevision,
                type: "request_snapshot",
            };
            if (!this.#queueAndSend(request, false, true)) {
                socket.close(1011, "liveness check failed");
                return;
            }
            this.#snapshotRequestId = request.id;
            this.#livenessRequestId = request.id;
            this.#armLivenessResponseTimer();
        }, LIVENESS_IDLE_MS);
    }
    #armLivenessResponseTimer() {
        if (this.#livenessResponseTimer !== null) {
            return;
        }
        const socket = this.#socket;
        const requestId = this.#livenessRequestId;
        if (socket === null ||
            socket.readyState !== WebSocket.OPEN ||
            requestId === null ||
            this.#permanentlyClosed ||
            this.#suspendedForBfcache) {
            return;
        }
        this.#livenessResponseTimer = globalThis.setTimeout(() => {
            this.#livenessResponseTimer = null;
            if (this.#socket === socket &&
                this.#livenessRequestId === requestId) {
                socket.close(1011, "liveness response timeout");
            }
        }, LIVENESS_RESPONSE_TIMEOUT_MS);
    }
    #clearLivenessResponseTimer() {
        if (this.#livenessResponseTimer !== null) {
            globalThis.clearTimeout(this.#livenessResponseTimer);
            this.#livenessResponseTimer = null;
        }
    }
    #clearLivenessTimer() {
        if (this.#livenessTimer !== null) {
            globalThis.clearTimeout(this.#livenessTimer);
            this.#livenessTimer = null;
        }
    }
    #emitStatus(phase, label, attempt, retryInMilliseconds) {
        this.#events.transportStatus({
            phase,
            label,
            attempt,
            retryInMilliseconds,
        });
    }
    #authenticationInFlight() {
        return (this.#sessionAuthentication !== null ||
            this.#tokenAuthentication !== null);
    }
    #handleOnline = () => {
        if (this.#permanentlyClosed ||
            this.#suspendedForBfcache ||
            this.#socket !== null ||
            this.#reconnectTimer !== null ||
            this.#authenticationInFlight()) {
            return;
        }
        this.#runReconnectAction();
    };
    #handleOffline = () => {
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#clearLivenessTimer();
        this.#clearLivenessResponseTimer();
        this.#synchronized = false;
        this.#authenticationAbortController?.abort();
        if (this.#socket !== null && this.#socket.readyState < WebSocket.CLOSING) {
            this.#socket.close(1001, "network offline");
        }
        this.#emitStatus("offline", "Offline; waiting for network", this.#reconnectAttempt, null);
    };
    #handlePageHide = (event) => {
        if (event.persisted) {
            this.#suspendedForBfcache = true;
            this.#synchronized = false;
            if (this.#sessionAuthentication !== null) {
                this.#reconnectAction = "session_auth";
            }
            else if (this.#tokenAuthentication !== null) {
                this.#reconnectAction = "token_auth";
            }
            this.#cancelAuthenticationRequest();
            this.#clearSynchronizationTimer();
            this.#clearStaleRecoveryTimer();
            this.#clearLivenessTimer();
            this.#clearLivenessResponseTimer();
            const socket = this.#socket;
            this.#socket = null;
            if (socket !== null && socket.readyState < WebSocket.CLOSING) {
                socket.close(1000, "page suspended");
            }
            this.#emitStatus("offline", "Page suspended; reconnecting when restored", this.#reconnectAttempt, null);
            return;
        }
        this.#permanentlyClosed = true;
        this.#suspendedForBfcache = false;
        this.#synchronized = false;
        this.#cancelAuthenticationRequest();
        this.#clearReconnectTimer();
        this.#clearSynchronizationTimer();
        this.#clearStaleRecoveryTimer();
        this.#clearLivenessTimer();
        this.#clearLivenessResponseTimer();
        this.#livenessRequestId = null;
        this.#websocketProtocol = null;
        this.#pendingToken = null;
        this.#reconnectAction = "connect";
        this.#sessionProofHasOpened = false;
        this.#sessionProofPreopenFailures = 0;
        const socket = this.#socket;
        this.#socket = null;
        if (socket !== null && socket.readyState < WebSocket.CLOSING) {
            socket.close(1000, "page hidden");
        }
    };
    #handlePageShow = (event) => {
        if (!event.persisted ||
            !this.#suspendedForBfcache ||
            this.#permanentlyClosed) {
            return;
        }
        this.#suspendedForBfcache = false;
        if (!globalThis.navigator.onLine) {
            this.#emitStatus("offline", "Offline; waiting for network", this.#reconnectAttempt, null);
            return;
        }
        this.#emitStatus("reconnecting", "Restoring connection", this.#reconnectAttempt, null);
        if (this.#reconnectTimer === null &&
            !this.#authenticationInFlight()) {
            this.#runReconnectAction();
        }
    };
}
