import { assertNever } from "./safety.js";
// Dependency-free compile-time fixture: adding or renaming any generated patch
// section must update both this mapping and the production reducer.
export const REDUCER_SECTION_FIXTURE = {
    session: "session",
    blocks: "blocks",
    activity: "activity",
    pending_approval: "pending_approval",
    pending_question: "pending_question",
    commands: "commands",
    sessions: "sessions",
    models: "models",
    themes: "themes",
    usage: "usage",
    status: "status",
    debugger: "debugger",
    overlay: "overlay",
};
export function fixtureSection(change) {
    switch (change.type) {
        case "session":
        case "blocks":
        case "activity":
        case "pending_approval":
        case "pending_question":
        case "commands":
        case "sessions":
        case "models":
        case "themes":
        case "usage":
        case "status":
        case "debugger":
        case "overlay":
            return REDUCER_SECTION_FIXTURE[change.type];
        default:
            return assertNever(change, "reducer fixture");
    }
}
export const WEB_COMMAND_FIXTURE = {
    invoke_command: true,
    send_prompt: true,
    stop: true,
    approve_tool_once: true,
    approve_tool_always: true,
    deny_tool: true,
    rename_session: true,
    answer_user: true,
    select_theme: true,
    select_model: true,
    new_session: true,
    resume_session: true,
    delete_session: true,
    wipe_sessions: true,
    select_history_entry: true,
    select_latest_file: true,
    set_skill_enabled: true,
    install_skill: true,
    select_system_prompt: true,
    save_system_prompt: true,
    set_loop_detection: true,
    set_agent_mode: true,
    run_lsp_action: true,
    clear_context: true,
    delete_python_runtime: true,
    dismiss_overlay: true,
    request_snapshot: true,
    quit: true,
};
export const STOP_COMMAND_FIXTURE = {
    type: "stop",
    session_id: "11111111-2222-4333-8444-555555555555",
    cancel_id: "cancel-fixture",
};
export const HISTORY_SELECTION_COMMAND_FIXTURE = {
    type: "select_history_entry",
    session_id: "11111111-2222-4333-8444-555555555555",
    entry_id: "history-fixture",
};
export function fixtureCommandType(command) {
    switch (command.type) {
        case "invoke_command":
        case "send_prompt":
        case "stop":
        case "approve_tool_once":
        case "approve_tool_always":
        case "deny_tool":
        case "rename_session":
        case "answer_user":
        case "select_theme":
        case "select_model":
        case "new_session":
        case "resume_session":
        case "delete_session":
        case "wipe_sessions":
        case "select_history_entry":
        case "select_latest_file":
        case "set_skill_enabled":
        case "install_skill":
        case "select_system_prompt":
        case "save_system_prompt":
        case "set_loop_detection":
        case "set_agent_mode":
        case "run_lsp_action":
        case "clear_context":
        case "delete_python_runtime":
        case "dismiss_overlay":
        case "request_snapshot":
        case "quit":
            return command.type;
        default:
            return assertNever(command, "command fixture");
    }
}
