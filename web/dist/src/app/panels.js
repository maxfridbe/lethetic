import { jsx as h } from "../../lib/snabbdom/build/index.js";
import { ICON_GLYPHS } from "../generated/contracts.js";
import { uiIcon } from "../icons.js";
import { jsonPreformattedThunk } from "../json.js";
import { boundedText } from "../safety.js";
import { MAX_DRAFT_LENGTH, MAX_SYSTEM_PROMPT_BYTES, MAX_SYSTEM_PROMPT_NAME_BYTES, PANEL_TITLES, approvalConfirmationKey, approvalIcon, approvalLabel, approvalUsesJsonHighlighting, choiceLabel, filteredCommands, hiddenApprovalConfirmationMessage, inputValue, lspActionIcon, lspActionLabel, orderedThemes, panelData, panelInstanceKey, safeThemeColor, utf8Length, } from "./helpers.js";
function emptyPanel(message) {
    return h("p", { attrs: { class: "empty-state" } }, message);
}
function commandPalette(context, snapshot) {
    const palette = context.state.palette ?? { query: "", selected: 0 };
    const commands = filteredCommands(snapshot, palette.query);
    const selected = Math.min(palette.selected, Math.max(0, commands.length - 1));
    if (context.state.palette !== null && palette.selected !== selected) {
        context.actions.clampPaletteSelection(selected);
    }
    return (h("div", { attrs: { class: "palette-content" } },
        h("label", { attrs: { for: "palette-search", class: "sr-only" } }, "Search commands"),
        h("input", { attrs: {
                id: "palette-search",
                type: "search",
                placeholder: "Search commands",
                autocomplete: "off",
                "aria-controls": "palette-list",
            }, props: { value: palette.query }, on: {
                input: (event) => context.actions.setPaletteQuery(inputValue(event)),
            } }),
        h("ul", { attrs: {
                id: "palette-list",
                class: "choice-list palette-command-surface",
                role: "listbox",
                tabindex: "0",
                "aria-label": "Commands",
                "aria-activedescendant": commands[selected]
                    ? `palette-command-${commands[selected].id}`
                    : "",
                "data-autofocus": "true",
            }, on: { keydown: context.actions.onPaletteKeyDown } }, commands.map((command, index) => {
            const disabled = !context.state.live || !command.enabled;
            return (h("li", { key: command.id, attrs: {
                    id: `palette-command-${command.id}`,
                    class: `choice-row${index === selected ? " is-active" : ""}${disabled ? " is-disabled" : ""}`,
                    role: "option",
                    "aria-selected": index === selected ? "true" : "false",
                    "aria-disabled": disabled ? "true" : "false",
                    title: command.disabled_reason ?? command.label,
                }, on: {
                    click: () => {
                        if (!disabled) {
                            context.actions.invokeCommand(command);
                        }
                    },
                } },
                h("span", { attrs: { class: "choice-icon", "aria-hidden": "true" } }, ICON_GLYPHS[command.icon]),
                h("span", { attrs: { class: "choice-main" } },
                    h("strong", null, command.label),
                    h("small", null, command.description)),
                command.accelerator === null ? null : (h("kbd", null, command.accelerator.toUpperCase()))));
        })),
        commands.length === 0 ? emptyPanel("No commands match.") : null));
}
function hotkeysPanel(_context, snapshot) {
    const data = panelData(snapshot, "hotkeys");
    if (data === null) {
        return emptyPanel("Hotkeys are not available.");
    }
    const shortcuts = [
        ...data.shortcuts,
        ...snapshot.commands
            .filter((command) => command.accelerator !== null)
            .map((command) => ({
            keys: command.accelerator?.toUpperCase() ?? "",
            label: `${command.label} (command palette focused)`,
        })),
    ];
    return (h("dl", { attrs: { class: "shortcut-list" } }, shortcuts.map((shortcut, index) => (h("div", { key: `shortcut-${index}`, attrs: { class: "shortcut-row" } },
        h("dt", null,
            h("kbd", null, shortcut.keys)),
        h("dd", null, shortcut.label))))));
}
function themesPanel(context, snapshot) {
    return (h("ul", { attrs: { class: "choice-list theme-list" } }, orderedThemes(snapshot).map((theme, index) => (h("li", { key: theme.theme_id },
        h("button", { attrs: {
                type: "button",
                class: `choice-row theme-choice${theme.selected ? " is-selected" : ""}`,
                "aria-pressed": theme.selected ? "true" : "false",
                "data-autofocus": index === 0 ? "true" : "false",
            }, props: { disabled: !context.state.live }, on: {
                click: () => context.actions.send({
                    type: "select_theme",
                    theme_id: theme.theme_id,
                }),
            } },
            h("span", { attrs: { class: "theme-swatch", "aria-hidden": "true" } },
                h("span", { style: {
                        backgroundColor: safeThemeColor(theme.colors.terminal_bg, "terminal_bg"),
                    } }),
                h("span", { style: {
                        backgroundColor: safeThemeColor(theme.colors.input_fg, "input_fg"),
                    } }),
                h("span", { style: {
                        backgroundColor: safeThemeColor(theme.colors.highlight_fg, "highlight_fg"),
                    } })),
            h("strong", null, theme.name),
            h("span", null, theme.selected ? "Current" : "Select")))))));
}
function historyPanel(context, snapshot) {
    const data = panelData(snapshot, "input_history");
    if (data === null) {
        return emptyPanel("Input history is not available.");
    }
    return (h("div", null,
        h("ul", { attrs: { class: "choice-list" } }, data.entries.map((entry, index) => (h("li", { key: entry.entry_id },
            h("button", { attrs: {
                    type: "button",
                    class: "choice-row history-choice",
                    "data-autofocus": index === 0 ? "true" : "false",
                }, props: { disabled: !context.state.live }, on: {
                    click: () => context.actions.selectHistoryEntry(snapshot, entry),
                } },
                h("span", null, entry.label)))))),
        data.has_more ? (h("p", { class: { "bounded-notice": true } }, "Older entries are omitted.")) : null));
}
function modesPanel(context, snapshot, dataType) {
    const data = dataType === "loop_modes"
        ? panelData(snapshot, "loop_modes")
        : panelData(snapshot, "agent_modes");
    if (data === null) {
        return emptyPanel("Mode choices are not available.");
    }
    return (h("ul", { attrs: { class: "choice-list" } }, data.modes.map((mode, index) => (h("li", { key: mode.mode_id },
        h("button", { attrs: {
                type: "button",
                class: `choice-row${mode.selected ? " is-selected" : ""}`,
                title: mode.disabled_reason ?? mode.label,
                "data-autofocus": index === 0 ? "true" : "false",
            }, props: { disabled: !context.state.live || !mode.enabled }, on: {
                click: () => context.actions.send(dataType === "loop_modes"
                    ? {
                        type: "set_loop_detection",
                        session_id: snapshot.session.session_id,
                        mode_id: mode.mode_id,
                    }
                    : {
                        type: "set_agent_mode",
                        session_id: snapshot.session.session_id,
                        mode_id: mode.mode_id,
                    }),
            } },
            h("strong", null, mode.label),
            h("span", null, mode.selected ? "Current" : mode.disabled_reason ?? "Select")))))));
}
function editorAvailabilityNotice(editorContent, contentTruncated) {
    if (contentTruncated) {
        return (h("p", { attrs: { class: "truncation-note" } }, "The editor projection was redacted or truncated. Remote editing and saving are disabled; select a prompt whose exact content can be shown."));
    }
    if (editorContent === null) {
        return (h("p", { attrs: { class: "bounded-notice" } }, "Select a saved prompt to open an exact remote editor. Creating a prompt remotely requires an exact editor projection first."));
    }
    return null;
}
function systemPromptPanel(context, snapshot) {
    const data = panelData(snapshot, "system_prompts");
    if (data === null) {
        return emptyPanel("System prompt data is not available.");
    }
    const editorUnavailable = data.editor_content === null || data.content_truncated;
    const { editorName, editorContent, live } = context.state;
    const trimmedName = editorName.trim();
    const saveDisabled = !live ||
        editorUnavailable ||
        trimmedName.length === 0 ||
        utf8Length(trimmedName) > MAX_SYSTEM_PROMPT_NAME_BYTES ||
        editorContent.length === 0 ||
        utf8Length(editorContent) > MAX_SYSTEM_PROMPT_BYTES;
    return (h("div", { attrs: { class: "system-prompt-panel" } },
        h("section", { attrs: { class: "prompt-choices" } },
            h("h3", null, "Saved prompts"),
            h("ul", { attrs: { class: "choice-list compact" } }, data.prompts.map((prompt, index) => (h("li", { key: prompt.prompt_id },
                h("button", { attrs: {
                        type: "button",
                        class: `choice-row${prompt.selected ? " is-selected" : ""}`,
                        "data-autofocus": editorUnavailable && index === 0 ? "true" : "false",
                    }, props: { disabled: !live }, on: {
                        click: () => context.actions.selectSystemPrompt(snapshot, prompt.prompt_id),
                    } },
                    h("span", null, prompt.label))))))),
        h("form", { attrs: { class: "prompt-editor", "aria-label": "System prompt editor" }, on: {
                submit: (event) => {
                    event.preventDefault();
                    context.actions.submitSystemPrompt(snapshot, editorUnavailable);
                },
            } },
            h("label", { attrs: { for: "prompt-name" } }, "Name"),
            h("input", { attrs: {
                    id: "prompt-name",
                    type: "text",
                    maxlength: "255",
                    autocomplete: "off",
                    "data-autofocus": editorUnavailable ? "false" : "true",
                }, props: {
                    value: editorName,
                    disabled: !live || editorUnavailable,
                }, on: {
                    input: (event) => context.actions.updateEditorName(inputValue(event)),
                } }),
            h("label", { attrs: { for: "prompt-content" } }, "Prompt content"),
            h("textarea", { attrs: {
                    id: "prompt-content",
                    rows: "18",
                    maxlength: String(MAX_DRAFT_LENGTH),
                }, props: {
                    value: editorContent,
                    disabled: !live || editorUnavailable,
                }, on: {
                    input: (event) => context.actions.updateEditorContent(inputValue(event)),
                } }),
            editorAvailabilityNotice(data.editor_content, data.content_truncated),
            h("div", { attrs: { class: "form-actions" } },
                h("button", { attrs: { type: "button" }, props: { disabled: !live || editorUnavailable }, on: { click: context.actions.createSystemPrompt } },
                    uiIcon("add"),
                    "New"),
                h("button", { attrs: { type: "submit", class: "primary-button" }, props: { disabled: saveDisabled } },
                    uiIcon("save"),
                    "Save")))));
}
function sessionsPanel(context, snapshot) {
    const { live } = context.state;
    return (h("div", { attrs: { class: "sessions-panel" } },
        h("div", { attrs: { class: "panel-toolbar" } },
            h("button", { attrs: {
                    type: "button",
                    class: "primary-button",
                    "data-autofocus": "true",
                }, props: { disabled: !live }, on: { click: () => context.actions.send({ type: "new_session" }) } },
                uiIcon("add"),
                "New session"),
            h("button", { attrs: { type: "button", class: "danger-button" }, props: { disabled: !live || snapshot.sessions.sessions.length === 0 }, on: {
                    click: () => context.actions.sendConfirmable({
                        type: "wipe_sessions",
                        confirmed: false,
                        confirmation_id: null,
                    }),
                } },
                uiIcon("trash"),
                "Delete all")),
        h("ul", { attrs: { class: "choice-list" } }, snapshot.sessions.sessions.map((session) => (h("li", { key: session.session_id, attrs: { class: "session-choice" } },
            h("button", { attrs: {
                    type: "button",
                    class: `choice-row${session.selected ? " is-selected" : ""}`,
                    "aria-current": session.selected ? "true" : "false",
                }, props: { disabled: !live || session.selected }, on: {
                    click: () => context.actions.send({
                        type: "resume_session",
                        session_id: session.session_id,
                    }),
                } },
                h("strong", null,
                    session.selected ? null : uiIcon("play"),
                    choiceLabel(session)),
                h("span", null, session.selected ? "Current" : "Resume")),
            h("button", { attrs: {
                    type: "button",
                    class: "danger-button compact-button",
                    "aria-label": `Delete ${choiceLabel(session)}`,
                }, props: { disabled: !live }, on: {
                    click: () => context.actions.sendConfirmable({
                        type: "delete_session",
                        session_id: session.session_id,
                        confirmed: false,
                        confirmation_id: null,
                    }),
                } },
                uiIcon("trash"),
                "Delete"))))),
        snapshot.sessions.has_more ? (h("p", { attrs: { class: "bounded-notice" } }, "Additional sessions are omitted.")) : null));
}
function nameSessionPanel(context, snapshot) {
    if (panelData(snapshot, "name_session") === null) {
        return emptyPanel("Session naming is not available.");
    }
    return (h("form", { attrs: { class: "simple-form" }, on: {
            submit: (event) => {
                event.preventDefault();
                context.actions.submitSessionName(snapshot);
            },
        } },
        h("label", { attrs: { for: "session-name" } }, "Session name"),
        h("input", { attrs: {
                id: "session-name",
                type: "text",
                maxlength: "255",
                autocomplete: "off",
                "data-autofocus": "true",
            }, props: { value: context.state.sessionName, disabled: !context.state.live }, on: {
                input: (event) => context.actions.updateSessionName(inputValue(event)),
            } }),
        h("div", { attrs: { class: "form-actions" } },
            h("button", { attrs: { type: "submit", class: "primary-button" }, props: { disabled: !context.state.live } },
                uiIcon("save"),
                "Save name"))));
}
function skillsPanel(context, snapshot) {
    const data = panelData(snapshot, "skills");
    if (data === null) {
        return emptyPanel("Skills are not available.");
    }
    const live = context.state.live;
    return (h("div", { attrs: { class: "skills-panel" } },
        data.message === null ? null : (h("p", { attrs: { class: "bounded-notice", role: "status" } }, data.message)),
        h("h3", null, "Found skills"),
        data.skills.length === 0 ? (h("p", null, "None yet. Install one below, or add a folder with SKILL.md to .lethetic/skills/.")) : (h("ul", { attrs: { class: "choice-list" } }, data.skills.map((skill, index) => (h("li", { key: skill.skill_id },
            h("button", { attrs: {
                    type: "button",
                    class: `choice-row${skill.enabled ? " is-current" : ""}`,
                    "aria-pressed": skill.enabled ? "true" : "false",
                    title: skill.description,
                    "data-autofocus": index === 0 ? "true" : "false",
                }, props: { disabled: !live }, on: {
                    click: () => context.actions.send({
                        type: "set_skill_enabled",
                        skill_id: skill.skill_id,
                        enabled: !skill.enabled,
                    }),
                } },
                h("span", null,
                    h("strong", null, skill.name),
                    " \u00B7 ",
                    skill.source,
                    h("br", null),
                    h("small", null, skill.description)),
                h("span", null, skill.enabled ? [uiIcon("check"), "On"] : "Off"))))))),
        h("h3", null, "Catalog \u00B7 Anthropic skills repository"),
        h("ul", { attrs: { class: "choice-list" } }, data.catalog.map((entry) => (h("li", { key: entry.entry_id, attrs: { class: "skill-catalog-row" } },
            h("span", null,
                h("a", { attrs: { href: entry.url, target: "_blank", rel: "noopener noreferrer" } },
                    h("strong", null, entry.name)),
                " · ",
                entry.summary,
                entry.proprietary ? h("small", null, " (proprietary license)") : null),
            h("button", { attrs: { type: "button", class: "choice-row" }, props: { disabled: !live || entry.installed || entry.installing }, on: {
                    click: () => context.actions.send({ type: "install_skill", entry_id: entry.entry_id }),
                } }, entry.installed
                ? [uiIcon("check"), "Installed"]
                : entry.installing
                    ? "Installing…"
                    : [uiIcon("download"), "Install"])))))));
}
function latestFilesPanel(context, snapshot) {
    const data = panelData(snapshot, "latest_files");
    if (data === null) {
        return emptyPanel("Latest files are not available.");
    }
    return (h("div", null,
        h("p", null, "Select a file to remove it from the latest-files context."),
        h("ul", { attrs: { class: "choice-list" } }, data.files.map((file, index) => (h("li", { key: file.file_id },
            h("button", { attrs: {
                    type: "button",
                    class: "choice-row",
                    "data-autofocus": index === 0 ? "true" : "false",
                }, props: { disabled: !context.state.live }, on: {
                    click: () => context.actions.send({
                        type: "select_latest_file",
                        session_id: snapshot.session.session_id,
                        file_id: file.file_id,
                    }),
                } },
                h("span", null, file.label),
                h("span", null,
                    uiIcon("trash"),
                    "Remove")))))),
        data.has_more ? (h("p", { attrs: { class: "bounded-notice" } }, "Additional files are omitted.")) : null));
}
function modelsPanel(context, snapshot) {
    const { live } = context.state;
    const autofocusIndex = snapshot.models.findIndex((model) => live && model.available && !model.selected);
    return (h("ul", { attrs: { class: "choice-list" } }, snapshot.models.map((model, index) => (h("li", { key: model.model_id },
        h("button", { attrs: {
                type: "button",
                class: `choice-row${model.selected ? " is-selected" : ""}`,
                title: model.available ? model.label : "Model unavailable",
                "data-autofocus": index === autofocusIndex ? "true" : "false",
            }, props: { disabled: !live || !model.available || model.selected }, on: {
                click: () => context.actions.send({
                    type: "select_model",
                    model_id: model.model_id,
                }),
            } },
            h("span", { attrs: { class: "choice-main" } },
                h("strong", null, model.label),
                h("small", null, `${model.model_name} · ${model.transport}`)),
            h("span", null, model.selected
                ? "Current"
                : model.available
                    ? "Select"
                    : "Unavailable")))))));
}
function lspPanel(context, snapshot) {
    const data = panelData(snapshot, "lsp_servers");
    if (data === null) {
        return emptyPanel("LSP server data is not available.");
    }
    return (h("ul", { attrs: { class: "choice-list" } }, data.servers.map((server, index) => (h("li", { key: server.server_id, attrs: { class: "lsp-choice" } },
        h("div", { attrs: { class: "choice-row static-choice" } },
            h("span", { attrs: { class: "choice-main" } },
                h("strong", null, server.label),
                h("small", null, server.state)),
            h("span", { attrs: { class: "inline-actions" } }, server.allowed_actions.map((action, actionIndex) => (h("button", { key: `${server.server_id}-${action}`, attrs: {
                    type: "button",
                    "data-autofocus": index === 0 && actionIndex === 0 ? "true" : "false",
                }, props: { disabled: !context.state.live }, on: {
                    click: () => context.actions.send({
                        type: "run_lsp_action",
                        session_id: snapshot.session.session_id,
                        server_id: server.server_id,
                        action,
                    }),
                } },
                uiIcon(lspActionIcon(action)),
                lspActionLabel(action)))))))))));
}
function approvalPanel(context, snapshot) {
    const approval = snapshot.pending_approval;
    if (approval === null) {
        return emptyPanel("This approval is no longer pending.");
    }
    const confirmationKey = approvalConfirmationKey(context.state.revision, approval);
    const candidate = context.state.approvalConfirmation;
    const confirmation = candidate?.key === confirmationKey &&
        approval.allowed_decisions.includes(candidate.decision)
        ? candidate
        : null;
    return (h("div", { attrs: { class: "approval-panel" } },
        h("dl", { attrs: { class: "metadata-list" } },
            h("div", null,
                h("dt", null, "Tool"),
                h("dd", null, approval.tool_name)),
            h("div", null,
                h("dt", null, "Description"),
                h("dd", null, approval.description)),
            h("div", null,
                h("dt", null, "Call ID"),
                h("dd", null, approval.tool_call_id))),
        jsonPreformattedThunk("approval", boundedText(approval.preview), approvalUsesJsonHighlighting(approval)),
        approval.preview_redacted ? (h("p", { attrs: { class: "truncation-note" } }, "Sensitive values were replaced in this preview. Any decision still targets the exact current server-held call.")) : null,
        approval.preview_truncated ? (h("p", { attrs: { class: "truncation-note" } }, "The preview omits a tail because it exceeded the browser display limit. Approval executes the complete current call, including unseen content.")) : null,
        approval.allowed_decisions.includes("approve_always") ? (h("p", { attrs: { class: "truncation-note" } }, "Always allow tools also permits all later tool calls under the current execution policy without another preview.")) : null,
        confirmation === null ? null : (h("div", { attrs: { class: "approval-confirmation", role: "alert" } },
            h("p", null, hiddenApprovalConfirmationMessage(confirmation.decision, approval)),
            approval.preview_truncated ? (h("p", null, "The unseen tail will be included in the executed call.")) : null,
            h("div", { attrs: { class: "form-actions" } },
                h("button", { attrs: { type: "button", "data-autofocus": "true" }, on: { click: context.actions.cancelApprovalConfirmation } },
                    uiIcon("close"),
                    "Cancel"),
                h("button", { attrs: { type: "button", class: "primary-button" }, props: { disabled: !context.state.live }, on: {
                        click: () => context.actions.confirmHiddenApproval(approval, confirmation),
                    } },
                    uiIcon(approvalIcon(confirmation.decision)),
                    confirmation.decision === "approve_always"
                        ? "Confirm always allow tools"
                        : "Confirm approve once")))),
        h("div", { attrs: { class: "form-actions approval-actions" } }, approval.allowed_decisions.map((decision) => (h("button", { key: JSON.stringify([
                approval.session_id,
                approval.approval_id,
                approval.tool_call_id,
                decision,
            ]), attrs: {
                type: "button",
                class: decision === "deny" ? "danger-button" : "primary-button",
                "data-autofocus": decision === "deny" && confirmation === null ? "true" : "false",
            }, props: { disabled: !context.state.live }, on: {
                click: () => context.actions.decideApproval(approval, decision),
            } },
            uiIcon(approvalIcon(decision)),
            approvalLabel(decision)))))));
}
function questionField(context, prompt, index, live) {
    const draft = context.actions.questionDraft(prompt.question_id);
    return (h("fieldset", { key: prompt.question_id, attrs: { class: "question-fieldset" } },
        h("legend", null, prompt.prompt),
        prompt.options.map((option) => {
            const selected = draft.selected.has(option.option_id);
            return (h("label", { key: option.option_id, attrs: { class: "option-row" } },
                h("input", { attrs: {
                        type: prompt.multiple ? "checkbox" : "radio",
                        "aria-label": option.label,
                    }, props: { checked: selected, disabled: !live }, on: {
                        change: () => context.actions.toggleQuestionOption(prompt, option.option_id, selected),
                    } }),
                h("span", null,
                    h("strong", null, option.label),
                    option.description === null ? null : (h("small", null, option.description)))));
        }),
        prompt.allows_other ? (h("label", { attrs: { class: "other-answer" } },
            h("span", null, prompt.options.length === 0 ? "Your answer" : "Other"),
            h("textarea", { attrs: {
                    rows: "4",
                    maxlength: String(MAX_DRAFT_LENGTH),
                    "data-autofocus": live && index === 0 && prompt.options.length === 0
                        ? "true"
                        : "false",
                }, props: { value: draft.other, disabled: !live }, on: {
                    input: (event) => context.actions.updateQuestionOther(prompt.question_id, inputValue(event)),
                } }))) : null));
}
function questionPanel(context, snapshot) {
    const question = snapshot.pending_question;
    if (question === null) {
        return emptyPanel("This question is no longer pending.");
    }
    const answerEnabled = context.state.live && !question.content_truncated;
    return (h("form", { attrs: { class: "question-form" }, on: {
            submit: (event) => {
                event.preventDefault();
                context.actions.submitAnswers(question);
            },
        } },
        question.content_truncated ? (h("p", { attrs: { class: "truncation-note" } }, "This question cannot be answered remotely because its exact content is unavailable. Cancel this request to continue, or review the original in the TUI.")) : null,
        question.questions.map((prompt, index) => questionField(context, prompt, index, answerEnabled)),
        h("div", { attrs: { class: "form-actions" } },
            h("button", { attrs: { type: "submit", class: "primary-button" }, props: {
                    disabled: !answerEnabled || !context.actions.answersComplete(question),
                } },
                uiIcon("send"),
                "Submit answer"),
            h("button", { attrs: { type: "button", class: "danger-button", "data-autofocus": question.content_truncated ? "true" : "false" }, props: { disabled: !context.state.live || !snapshot.activity.cancellable || snapshot.activity.cancel_id === null }, on: { click: context.actions.cancelQuestion } },
                uiIcon("stop"),
                "Cancel request"))));
}
function editorDiscardConfirmation(context) {
    return (h("div", { attrs: { class: "confirmation-content" } },
        h("h3", null, "Discard unsaved system prompt changes?"),
        h("p", null, "The local editor draft will be lost."),
        h("div", { attrs: { class: "form-actions" } },
            h("button", { attrs: { type: "button", "data-autofocus": "true" }, on: { click: context.actions.cancelEditorDiscard } },
                uiIcon("edit"),
                "Keep editing"),
            h("button", { attrs: { type: "button", class: "danger-button" }, on: { click: () => context.actions.close(true) } },
                uiIcon("trash"),
                "Discard"))));
}
function confirmationPanel(context, snapshot) {
    if (context.state.discardEditorConfirmation) {
        return editorDiscardConfirmation(context);
    }
    const data = panelData(snapshot, "confirmation");
    if (data === null) {
        return emptyPanel("No confirmation is pending.");
    }
    const confirmation = data.confirmation;
    const matches = context.actions.confirmationMatches(confirmation);
    return (h("div", { attrs: { class: "confirmation-content" } },
        h("h3", null, confirmation.title),
        h("p", null, confirmation.message),
        !matches ? (h("p", { attrs: { class: "truncation-note" } }, "This browser does not retain the exact initiating payload. Close this dialog and start the action again.")) : null,
        h("div", { attrs: { class: "form-actions" } },
            h("button", { attrs: { type: "button", "data-autofocus": "true" }, on: { click: () => context.actions.close() } },
                uiIcon("close"),
                "Cancel"),
            h("button", { attrs: { type: "button", class: "danger-button" }, props: { disabled: !context.state.live || !matches }, on: { click: () => context.actions.confirm(confirmation) } },
                uiIcon("warning"),
                "Confirm"))));
}
const PANEL_RENDERERS = {
    command_palette: commandPalette,
    hotkeys: hotkeysPanel,
    themes: themesPanel,
    input_history: historyPanel,
    loop_detection: (context, snapshot) => modesPanel(context, snapshot, "loop_modes"),
    system_prompt: systemPromptPanel,
    sessions: sessionsPanel,
    name_session: nameSessionPanel,
    latest_files: latestFilesPanel,
    models: modelsPanel,
    lsp_servers: lspPanel,
    agent_mode: (context, snapshot) => modesPanel(context, snapshot, "agent_modes"),
    tool_approval: approvalPanel,
    ask_user: questionPanel,
    confirmation: confirmationPanel,
    skills: skillsPanel,
};
function panelBody(context) {
    const { snapshot, panel } = context.state;
    if (snapshot === null) {
        return emptyPanel("Waiting for synchronized state.");
    }
    return PANEL_RENDERERS[panel](context, snapshot);
}
export function renderPanel(context) {
    const { panel, snapshot } = context.state;
    return (h("div", { key: panelInstanceKey(panel, snapshot), attrs: { class: "overlay-backdrop", role: "presentation" }, on: {
            mousedown: (event) => {
                if (event.target === event.currentTarget) {
                    context.actions.close();
                }
            },
        } },
        h("section", { attrs: {
                class: `overlay-panel panel-${panel}`,
                role: "dialog",
                "aria-modal": "true",
                "aria-labelledby": "overlay-title",
                "data-focus-trap": "true",
            } },
            h("header", { attrs: { class: "overlay-header" } },
                h("h2", { attrs: { id: "overlay-title" } }, PANEL_TITLES[panel]),
                h("button", { attrs: {
                        type: "button",
                        class: "icon-button",
                        "aria-label": "Close overlay",
                    }, on: { click: () => context.actions.close() } }, uiIcon("close"))),
            h("div", { attrs: { class: "overlay-body" } }, panelBody(context)))));
}
