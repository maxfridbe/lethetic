import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { boundedText } from "../safety.js";
import { activityLabel, projectionLossMessages } from "./helpers.js";
function policySourceLabel(source) {
    switch (source) {
        case "config":
            return "config";
        case "global":
            return "global";
        case "project":
            return "project";
        case "one_time":
            return "one-time";
        case "cli_locked":
            return "cli-locked";
    }
}
export function pythonStatusLabel(python) {
    const source = policySourceLabel(python.policy_source);
    if (python.profile === "general") {
        return `General / ${source}`;
    }
    if (python.target === null) {
        return `Python unresolved / ${source}`;
    }
    if (python.target === "host") {
        return `Python Host / ${source}`;
    }
    const backend = python.backend === "bubblewrap"
        ? "Bubblewrap"
        : python.backend === "podman"
            ? "Podman"
            : "unresolved";
    const network = python.network === "none"
        ? "network none"
        : python.network === "public_only"
            ? "network public-only"
            : python.network === "full"
                ? "network full"
                : "network unresolved";
    const workspace = python.workspace_access === "read_only"
        ? "workspace read-only"
        : python.workspace_access === "read_write"
            ? "workspace read/write"
            : "workspace unresolved";
    return `Python ${backend}, ${network}, ${workspace}, ${python.grant_count} grant(s) / ${source}`;
}
function providerTransportLabel(transport) {
    switch (transport) {
        case "open_ai_chat_completions":
            return "OpenAI-compatible";
        case "claude_code_proxy":
            return "Claude Code proxy";
    }
}
function contextSourceLabel(source) {
    switch (source) {
        case "server_usage":
            return "server usage";
        case "server_prompt":
            return "server prompt";
        case "local_estimate":
            return "local estimate";
    }
}
export function usageBreakdown(usage) {
    const marker = usage.breakdown_complete ? "" : "*";
    return `${usage.total_tokens} total (uncached ${usage.uncached_input_tokens}, cache-read ${usage.cache_read_input_tokens}, cache-write ${usage.cache_creation_input_tokens}, output ${usage.output_tokens})${marker}`;
}
export function applicationStatusFields(snapshot) {
    const { status, usage } = snapshot;
    const container = status.python.container;
    const fields = [
        { label: "Stop", value: boundedText(status.stop_reason) },
        { label: "Activity", value: activityLabel(snapshot.activity.kind) },
        { label: "Model", value: boundedText(status.model_label) },
        {
            label: "Provider",
            value: `${boundedText(status.provider_label)} (${providerTransportLabel(status.provider_transport)})`,
        },
        { label: "Python", value: pythonStatusLabel(status.python) },
        {
            label: "Container",
            value: container === null
                ? "none"
                : `${boundedText(container.name)} (${container.kind}, ${container.active ? "active" : "inactive"})`,
        },
        {
            label: "Context",
            value: `${status.context_tokens}/${status.context_limit_tokens} (${contextSourceLabel(status.context_source)})`,
        },
        {
            label: "Request",
            value: status.request_usage === null
                ? "—"
                : usageBreakdown(status.request_usage),
        },
        {
            label: "Turn",
            value: `${usage.latest_turn.usage.total_tokens} tokens`,
        },
        {
            label: "Turn API-eq",
            value: usage.latest_turn.estimated_cost?.display ?? "—",
        },
        {
            label: "Session",
            value: `${usage.session.usage.total_tokens} tokens`,
        },
        {
            label: "Session API-eq",
            value: usage.session.estimated_cost?.display ?? "—",
        },
        {
            label: "Rate",
            value: `${status.tokens_per_second ?? "—"} tok/s; ${status.prompt_tokens_per_second ?? "—"} prompt tok/s`,
        },
        { label: "Memory", value: `${status.memory_mebibytes} MiB` },
        { label: "Files", value: String(status.file_count) },
        { label: "Blocks", value: String(status.visible_block_count) },
        { label: "Git", value: status.git_state },
    ];
    fields.push(...projectionLossMessages([status.stop_reason_loss]).map((message) => ({
        label: "Status disclosure",
        value: message,
        warning: true,
    })));
    return fields;
}
export function transportStatusFields(state) {
    const fields = [
        {
            label: "Browser transport",
            value: `${state.transportStatus.phase}: ${boundedText(state.transportStatus.label)}`,
        },
        {
            label: "Mirror",
            value: state.live ? "synchronized" : "not synchronized",
        },
        {
            label: "Retry",
            value: state.transportStatus.retryInMilliseconds === null
                ? `none (attempt ${state.transportStatus.attempt})`
                : `${Math.ceil(state.transportStatus.retryInMilliseconds / 1000)}s (attempt ${state.transportStatus.attempt})`,
        },
        { label: "Pending", value: String(state.pendingCount) },
        {
            label: "Mirror bounds",
            value: state.snapshot?.blocks.truncated === true
                ? "server-bounded"
                : "complete available projection",
        },
    ];
    if (state.remoteReason !== null &&
        state.remoteReason !== state.transportStatus.label) {
        fields.push({
            label: "Mirror detail",
            value: boundedText(state.remoteReason),
            warning: true,
        });
    }
    return fields;
}
function renderFields(className, label, fields) {
    return (h("section", { attrs: { class: className, "aria-label": label } }, fields.map((field, index) => (h("span", { key: `${field.label}-${index}`, attrs: {
            class: `status-field${field.warning === true ? " is-warning" : ""}`,
        } },
        h("strong", null, field.label),
        h("span", null, field.value))))));
}
export function renderApplicationStatus(snapshot) {
    return renderFields("status-level", "Application and runtime status", snapshot === null
        ? [{ label: "Application", value: "Waiting for state" }]
        : applicationStatusFields(snapshot));
}
export function renderTransportStatus(state) {
    return renderFields("footer-level transport-level", "Browser transport and mirror status", transportStatusFields(state));
}
