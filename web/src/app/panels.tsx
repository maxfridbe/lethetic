import { jsx as h } from "../../lib/snabbdom/build/index.js";
import type { VNode } from "../../lib/snabbdom/build/index.js";
import { ICON_GLYPHS } from "../generated/contracts.js";
import type {
  PendingApprovalView,
  QuestionPromptView,
  WebAppSnapshot,
} from "../generated/contracts.js";
import { uiIcon } from "../icons.js";
import { jsonPreformattedThunk } from "../json.js";
import { boundedText } from "../safety.js";
import {
  MAX_DRAFT_LENGTH,
  MAX_SYSTEM_PROMPT_BYTES,
  MAX_SYSTEM_PROMPT_NAME_BYTES,
  PANEL_TITLES,
  approvalConfirmationKey,
  approvalIcon,
  approvalLabel,
  approvalUsesJsonHighlighting,
  choiceLabel,
  filteredCommands,
  hiddenApprovalConfirmationMessage,
  inputValue,
  lspActionIcon,
  lspActionLabel,
  orderedThemes,
  panelData,
  panelInstanceKey,
  safeThemeColor,
  utf8Length,
} from "./helpers.js";
import type { PanelViewContext } from "./state.js";

function emptyPanel(message: string): VNode {
  return <p attrs={{ class: "empty-state" }}>{message}</p>;
}

type PanelRenderer = (
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
) => VNode;

function commandPalette(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const palette = context.state.palette ?? { query: "", selected: 0 };
  const commands = filteredCommands(snapshot, palette.query);
  const selected = Math.min(
    palette.selected,
    Math.max(0, commands.length - 1),
  );
  if (context.state.palette !== null && palette.selected !== selected) {
    context.actions.clampPaletteSelection(selected);
  }
  return (
    <div attrs={{ class: "palette-content" }}>
      <label attrs={{ for: "palette-search", class: "sr-only" }}>
        Search commands
      </label>
      <input
        attrs={{
          id: "palette-search",
          type: "search",
          placeholder: "Search commands",
          autocomplete: "off",
          "aria-controls": "palette-list",
        }}
        props={{ value: palette.query }}
        on={{
          input: (event: InputEvent) =>
            context.actions.setPaletteQuery(inputValue(event)),
        }}
      />
      <ul
        attrs={{
          id: "palette-list",
          class: "choice-list palette-command-surface",
          role: "listbox",
          tabindex: "0",
          "aria-label": "Commands",
          "aria-activedescendant": commands[selected]
            ? `palette-command-${commands[selected].id}`
            : "",
          "data-autofocus": "true",
        }}
        on={{ keydown: context.actions.onPaletteKeyDown }}
      >
        {commands.map((command, index) => {
          const disabled = !context.state.live || !command.enabled;
          return (
            <li
              key={command.id}
              attrs={{
                id: `palette-command-${command.id}`,
                class: `choice-row${index === selected ? " is-active" : ""}${disabled ? " is-disabled" : ""}`,
                role: "option",
                "aria-selected": index === selected ? "true" : "false",
                "aria-disabled": disabled ? "true" : "false",
                title: command.disabled_reason ?? command.label,
              }}
              on={{
                click: () => {
                  if (!disabled) {
                    context.actions.invokeCommand(command);
                  }
                },
              }}
            >
              <span attrs={{ class: "choice-icon", "aria-hidden": "true" }}>
                {ICON_GLYPHS[command.icon]}
              </span>
              <span attrs={{ class: "choice-main" }}>
                <strong>{command.label}</strong>
                <small>{command.description}</small>
              </span>
              {command.accelerator === null ? null : (
                <kbd>{command.accelerator.toUpperCase()}</kbd>
              )}
            </li>
          );
        })}
      </ul>
      {commands.length === 0 ? emptyPanel("No commands match.") : null}
    </div>
  );
}

function hotkeysPanel(
  _context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
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
  return (
    <dl attrs={{ class: "shortcut-list" }}>
      {shortcuts.map((shortcut, index) => (
        <div key={`shortcut-${index}`} attrs={{ class: "shortcut-row" }}>
          <dt><kbd>{shortcut.keys}</kbd></dt>
          <dd>{shortcut.label}</dd>
        </div>
      ))}
    </dl>
  );
}

function themesPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  return (
    <ul attrs={{ class: "choice-list theme-list" }}>
      {orderedThemes(snapshot).map((theme, index) => (
        <li key={theme.theme_id}>
          <button
            attrs={{
              type: "button",
              class: `choice-row theme-choice${theme.selected ? " is-selected" : ""}`,
              "aria-pressed": theme.selected ? "true" : "false",
              "data-autofocus": index === 0 ? "true" : "false",
            }}
            props={{ disabled: !context.state.live }}
            on={{
              click: () =>
                context.actions.send({
                  type: "select_theme",
                  theme_id: theme.theme_id,
                }),
            }}
          >
            <span attrs={{ class: "theme-swatch", "aria-hidden": "true" }}>
              <span
                style={{
                  backgroundColor: safeThemeColor(
                    theme.colors.terminal_bg,
                    "terminal_bg",
                  ),
                }}
              ></span>
              <span
                style={{
                  backgroundColor: safeThemeColor(
                    theme.colors.input_fg,
                    "input_fg",
                  ),
                }}
              ></span>
              <span
                style={{
                  backgroundColor: safeThemeColor(
                    theme.colors.highlight_fg,
                    "highlight_fg",
                  ),
                }}
              ></span>
            </span>
            <strong>{theme.name}</strong>
            <span>{theme.selected ? "Current" : "Select"}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function historyPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const data = panelData(snapshot, "input_history");
  if (data === null) {
    return emptyPanel("Input history is not available.");
  }
  return (
    <div>
      <ul attrs={{ class: "choice-list" }}>
        {data.entries.map((entry, index) => (
          <li key={entry.entry_id}>
            <button
              attrs={{
                type: "button",
                class: "choice-row history-choice",
                "data-autofocus": index === 0 ? "true" : "false",
              }}
              props={{ disabled: !context.state.live }}
              on={{
                click: () => context.actions.selectHistoryEntry(snapshot, entry),
              }}
            >
              <span>{entry.label}</span>
            </button>
          </li>
        ))}
      </ul>
      {data.has_more ? (
        <p class={{ "bounded-notice": true }}>Older entries are omitted.</p>
      ) : null}
    </div>
  );
}

function modesPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
  dataType: "loop_modes" | "agent_modes",
): VNode {
  const data =
    dataType === "loop_modes"
      ? panelData(snapshot, "loop_modes")
      : panelData(snapshot, "agent_modes");
  if (data === null) {
    return emptyPanel("Mode choices are not available.");
  }
  return (
    <ul attrs={{ class: "choice-list" }}>
      {data.modes.map((mode, index) => (
        <li key={mode.mode_id}>
          <button
            attrs={{
              type: "button",
              class: `choice-row${mode.selected ? " is-selected" : ""}`,
              title: mode.disabled_reason ?? mode.label,
              "data-autofocus": index === 0 ? "true" : "false",
            }}
            props={{ disabled: !context.state.live || !mode.enabled }}
            on={{
              click: () =>
                context.actions.send(
                  dataType === "loop_modes"
                    ? {
                        type: "set_loop_detection",
                        session_id: snapshot.session.session_id,
                        mode_id: mode.mode_id,
                      }
                    : {
                        type: "set_agent_mode",
                        session_id: snapshot.session.session_id,
                        mode_id: mode.mode_id,
                      },
                ),
            }}
          >
            <strong>{mode.label}</strong>
            <span>
              {mode.selected ? "Current" : mode.disabled_reason ?? "Select"}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function editorAvailabilityNotice(
  editorContent: string | null,
  contentTruncated: boolean,
): VNode | null {
  if (contentTruncated) {
    return (
      <p attrs={{ class: "truncation-note" }}>
        The editor projection was redacted or truncated. Remote editing and
        saving are disabled; select a prompt whose exact content can be shown.
      </p>
    );
  }
  if (editorContent === null) {
    return (
      <p attrs={{ class: "bounded-notice" }}>
        Select a saved prompt to open an exact remote editor. Creating a prompt
        remotely requires an exact editor projection first.
      </p>
    );
  }
  return null;
}

function systemPromptPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const data = panelData(snapshot, "system_prompts");
  if (data === null) {
    return emptyPanel("System prompt data is not available.");
  }
  const editorUnavailable =
    data.editor_content === null || data.content_truncated;
  const { editorName, editorContent, live } = context.state;
  const trimmedName = editorName.trim();
  const saveDisabled =
    !live ||
    editorUnavailable ||
    trimmedName.length === 0 ||
    utf8Length(trimmedName) > MAX_SYSTEM_PROMPT_NAME_BYTES ||
    editorContent.length === 0 ||
    utf8Length(editorContent) > MAX_SYSTEM_PROMPT_BYTES;
  return (
    <div attrs={{ class: "system-prompt-panel" }}>
      <section attrs={{ class: "prompt-choices" }}>
        <h3>Saved prompts</h3>
        <ul attrs={{ class: "choice-list compact" }}>
          {data.prompts.map((prompt, index) => (
            <li key={prompt.prompt_id}>
              <button
                attrs={{
                  type: "button",
                  class: `choice-row${prompt.selected ? " is-selected" : ""}`,
                  "data-autofocus":
                    editorUnavailable && index === 0 ? "true" : "false",
                }}
                props={{ disabled: !live }}
                on={{
                  click: () =>
                    context.actions.selectSystemPrompt(
                      snapshot,
                      prompt.prompt_id,
                    ),
                }}
              >
                <span>{prompt.label}</span>
              </button>
            </li>
          ))}
        </ul>
      </section>
      <form
        attrs={{ class: "prompt-editor", "aria-label": "System prompt editor" }}
        on={{
          submit: (event: SubmitEvent) => {
            event.preventDefault();
            context.actions.submitSystemPrompt(snapshot, editorUnavailable);
          },
        }}
      >
        <label attrs={{ for: "prompt-name" }}>Name</label>
        <input
          attrs={{
            id: "prompt-name",
            type: "text",
            maxlength: "255",
            autocomplete: "off",
            "data-autofocus": editorUnavailable ? "false" : "true",
          }}
          props={{
            value: editorName,
            disabled: !live || editorUnavailable,
          }}
          on={{
            input: (event: InputEvent) =>
              context.actions.updateEditorName(inputValue(event)),
          }}
        />
        <label attrs={{ for: "prompt-content" }}>Prompt content</label>
        <textarea
          attrs={{
            id: "prompt-content",
            rows: "18",
            maxlength: String(MAX_DRAFT_LENGTH),
          }}
          props={{
            value: editorContent,
            disabled: !live || editorUnavailable,
          }}
          on={{
            input: (event: InputEvent) =>
              context.actions.updateEditorContent(inputValue(event)),
          }}
        ></textarea>
        {editorAvailabilityNotice(data.editor_content, data.content_truncated)}
        <div attrs={{ class: "form-actions" }}>
          <button
            attrs={{ type: "button" }}
            props={{ disabled: !live || editorUnavailable }}
            on={{ click: context.actions.createSystemPrompt }}
          >
            {uiIcon("add")}
            New
          </button>
          <button
            attrs={{ type: "submit", class: "primary-button" }}
            props={{ disabled: saveDisabled }}
          >
            {uiIcon("save")}
            Save
          </button>
        </div>
      </form>
    </div>
  );
}

function sessionsPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const { live } = context.state;
  return (
    <div attrs={{ class: "sessions-panel" }}>
      <div attrs={{ class: "panel-toolbar" }}>
        <button
          attrs={{
            type: "button",
            class: "primary-button",
            "data-autofocus": "true",
          }}
          props={{ disabled: !live }}
          on={{ click: () => context.actions.send({ type: "new_session" }) }}
        >
          {uiIcon("add")}
          New session
        </button>
        <button
          attrs={{ type: "button", class: "danger-button" }}
          props={{ disabled: !live || snapshot.sessions.sessions.length === 0 }}
          on={{
            click: () =>
              context.actions.sendConfirmable({
                type: "wipe_sessions",
                confirmed: false,
                confirmation_id: null,
              }),
          }}
        >
          {uiIcon("trash")}
          Delete all
        </button>
      </div>
      <ul attrs={{ class: "choice-list" }}>
        {snapshot.sessions.sessions.map((session) => (
          <li key={session.session_id} attrs={{ class: "session-choice" }}>
            <button
              attrs={{
                type: "button",
                class: `choice-row${session.selected ? " is-selected" : ""}`,
                "aria-current": session.selected ? "true" : "false",
              }}
              props={{ disabled: !live || session.selected }}
              on={{
                click: () =>
                  context.actions.send({
                    type: "resume_session",
                    session_id: session.session_id,
                  }),
              }}
            >
              <strong>
                {session.selected ? null : uiIcon("play")}
                {choiceLabel(session)}
              </strong>
              <span>{session.selected ? "Current" : "Resume"}</span>
            </button>
            <button
              attrs={{
                type: "button",
                class: "danger-button compact-button",
                "aria-label": `Delete ${choiceLabel(session)}`,
              }}
              props={{ disabled: !live }}
              on={{
                click: () =>
                  context.actions.sendConfirmable({
                    type: "delete_session",
                    session_id: session.session_id,
                    confirmed: false,
                    confirmation_id: null,
                  }),
              }}
            >
              {uiIcon("trash")}
              Delete
            </button>
          </li>
        ))}
      </ul>
      {snapshot.sessions.has_more ? (
        <p attrs={{ class: "bounded-notice" }}>Additional sessions are omitted.</p>
      ) : null}
    </div>
  );
}

function nameSessionPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  if (panelData(snapshot, "name_session") === null) {
    return emptyPanel("Session naming is not available.");
  }
  return (
    <form
      attrs={{ class: "simple-form" }}
      on={{
        submit: (event: SubmitEvent) => {
          event.preventDefault();
          context.actions.submitSessionName(snapshot);
        },
      }}
    >
      <label attrs={{ for: "session-name" }}>Session name</label>
      <input
        attrs={{
          id: "session-name",
          type: "text",
          maxlength: "255",
          autocomplete: "off",
          "data-autofocus": "true",
        }}
        props={{ value: context.state.sessionName, disabled: !context.state.live }}
        on={{
          input: (event: InputEvent) =>
            context.actions.updateSessionName(inputValue(event)),
        }}
      />
      <div attrs={{ class: "form-actions" }}>
        <button
          attrs={{ type: "submit", class: "primary-button" }}
          props={{ disabled: !context.state.live }}
        >
          {uiIcon("save")}
          Save name
        </button>
      </div>
    </form>
  );
}

function skillsPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const data = panelData(snapshot, "skills");
  if (data === null) {
    return emptyPanel("Skills are not available.");
  }
  const live = context.state.live;
  return (
    <div attrs={{ class: "skills-panel" }}>
      {data.message === null ? null : (
        <p attrs={{ class: "bounded-notice", role: "status" }}>{data.message}</p>
      )}
      <h3>Found skills</h3>
      {data.skills.length === 0 ? (
        <p>None yet. Install one below, or add a folder with SKILL.md to .lethetic/skills/.</p>
      ) : (
        <ul attrs={{ class: "choice-list" }}>
          {data.skills.map((skill, index) => (
            <li key={skill.skill_id}>
              <button
                attrs={{
                  type: "button",
                  class: `choice-row${skill.enabled ? " is-current" : ""}`,
                  "aria-pressed": skill.enabled ? "true" : "false",
                  title: skill.description,
                  "data-autofocus": index === 0 ? "true" : "false",
                }}
                props={{ disabled: !live }}
                on={{
                  click: () =>
                    context.actions.send({
                      type: "set_skill_enabled",
                      skill_id: skill.skill_id,
                      enabled: !skill.enabled,
                    }),
                }}
              >
                <span>
                  <strong>{skill.name}</strong> · {skill.source}
                  <br />
                  <small>{skill.description}</small>
                </span>
                <span>{skill.enabled ? [uiIcon("check"), "On"] : "Off"}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <h3>Catalog · Anthropic skills repository</h3>
      <ul attrs={{ class: "choice-list" }}>
        {data.catalog.map((entry) => (
          <li key={entry.entry_id} attrs={{ class: "skill-catalog-row" }}>
            <span>
              <a attrs={{ href: entry.url, target: "_blank", rel: "noopener noreferrer" }}>
                <strong>{entry.name}</strong>
              </a>
              {" · "}
              {entry.summary}
              {entry.proprietary ? <small> (proprietary license)</small> : null}
            </span>
            <button
              attrs={{ type: "button", class: "choice-row" }}
              props={{ disabled: !live || entry.installed || entry.installing }}
              on={{
                click: () =>
                  context.actions.send({ type: "install_skill", entry_id: entry.entry_id }),
              }}
            >
              {entry.installed
                ? [uiIcon("check"), "Installed"]
                : entry.installing
                  ? "Installing…"
                  : [uiIcon("download"), "Install"]}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function latestFilesPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const data = panelData(snapshot, "latest_files");
  if (data === null) {
    return emptyPanel("Latest files are not available.");
  }
  return (
    <div>
      <p>Select a file to remove it from the latest-files context.</p>
      <ul attrs={{ class: "choice-list" }}>
        {data.files.map((file, index) => (
          <li key={file.file_id}>
            <button
              attrs={{
                type: "button",
                class: "choice-row",
                "data-autofocus": index === 0 ? "true" : "false",
              }}
              props={{ disabled: !context.state.live }}
              on={{
                click: () =>
                  context.actions.send({
                    type: "select_latest_file",
                    session_id: snapshot.session.session_id,
                    file_id: file.file_id,
                  }),
              }}
            >
              <span>{file.label}</span>
              <span>{uiIcon("trash")}Remove</span>
            </button>
          </li>
        ))}
      </ul>
      {data.has_more ? (
        <p attrs={{ class: "bounded-notice" }}>Additional files are omitted.</p>
      ) : null}
    </div>
  );
}

function modelsPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const { live } = context.state;
  const autofocusIndex = snapshot.models.findIndex(
    (model) => live && model.available && !model.selected,
  );
  return (
    <ul attrs={{ class: "choice-list" }}>
      {snapshot.models.map((model, index) => (
        <li key={model.model_id}>
          <button
            attrs={{
              type: "button",
              class: `choice-row${model.selected ? " is-selected" : ""}`,
              title: model.available ? model.label : "Model unavailable",
              "data-autofocus": index === autofocusIndex ? "true" : "false",
            }}
            props={{ disabled: !live || !model.available || model.selected }}
            on={{
              click: () =>
                context.actions.send({
                  type: "select_model",
                  model_id: model.model_id,
                }),
            }}
          >
            <span attrs={{ class: "choice-main" }}>
              <strong>{model.label}</strong>
              <small>{`${model.model_name} · ${model.transport}`}</small>
            </span>
            <span>
              {model.selected
                ? "Current"
                : model.available
                  ? "Select"
                  : "Unavailable"}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function lspPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const data = panelData(snapshot, "lsp_servers");
  if (data === null) {
    return emptyPanel("LSP server data is not available.");
  }
  return (
    <ul attrs={{ class: "choice-list" }}>
      {data.servers.map((server, index) => (
        <li key={server.server_id} attrs={{ class: "lsp-choice" }}>
          <div attrs={{ class: "choice-row static-choice" }}>
            <span attrs={{ class: "choice-main" }}>
              <strong>{server.label}</strong>
              <small>{server.state}</small>
            </span>
            <span attrs={{ class: "inline-actions" }}>
              {server.allowed_actions.map((action, actionIndex) => (
                <button
                  key={`${server.server_id}-${action}`}
                  attrs={{
                    type: "button",
                    "data-autofocus":
                      index === 0 && actionIndex === 0 ? "true" : "false",
                  }}
                  props={{ disabled: !context.state.live }}
                  on={{
                    click: () =>
                      context.actions.send({
                        type: "run_lsp_action",
                        session_id: snapshot.session.session_id,
                        server_id: server.server_id,
                        action,
                      }),
                  }}
                >
                  {uiIcon(lspActionIcon(action))}
                  {lspActionLabel(action)}
                </button>
              ))}
            </span>
          </div>
        </li>
      ))}
    </ul>
  );
}

function approvalPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const approval = snapshot.pending_approval;
  if (approval === null) {
    return emptyPanel("This approval is no longer pending.");
  }
  const confirmationKey = approvalConfirmationKey(
    context.state.revision,
    approval,
  );
  const candidate = context.state.approvalConfirmation;
  const confirmation =
    candidate?.key === confirmationKey &&
    approval.allowed_decisions.includes(candidate.decision)
      ? candidate
      : null;
  return (
    <div attrs={{ class: "approval-panel" }}>
      <dl attrs={{ class: "metadata-list" }}>
        <div><dt>Tool</dt><dd>{approval.tool_name}</dd></div>
        <div><dt>Description</dt><dd>{approval.description}</dd></div>
        <div><dt>Call ID</dt><dd>{approval.tool_call_id}</dd></div>
      </dl>
      {jsonPreformattedThunk(
        "approval",
        boundedText(approval.preview),
        approvalUsesJsonHighlighting(approval),
      )}
      {approval.preview_redacted ? (
        <p attrs={{ class: "truncation-note" }}>
          Sensitive values were replaced in this preview. Any decision still
          targets the exact current server-held call.
        </p>
      ) : null}
      {approval.preview_truncated ? (
        <p attrs={{ class: "truncation-note" }}>
          The preview omits a tail because it exceeded the browser display
          limit. Approval executes the complete current call, including unseen
          content.
        </p>
      ) : null}
      {approval.allowed_decisions.includes("approve_always") ? (
        <p attrs={{ class: "truncation-note" }}>
          Always allow tools also permits all later tool calls under the current
          execution policy without another preview.
        </p>
      ) : null}
      {confirmation === null ? null : (
        <div attrs={{ class: "approval-confirmation", role: "alert" }}>
          <p>
            {hiddenApprovalConfirmationMessage(
              confirmation.decision,
              approval,
            )}
          </p>
          {approval.preview_truncated ? (
            <p>The unseen tail will be included in the executed call.</p>
          ) : null}
          <div attrs={{ class: "form-actions" }}>
            <button
              attrs={{ type: "button", "data-autofocus": "true" }}
              on={{ click: context.actions.cancelApprovalConfirmation }}
            >
              {uiIcon("close")}
              Cancel
            </button>
            <button
              attrs={{ type: "button", class: "primary-button" }}
              props={{ disabled: !context.state.live }}
              on={{
                click: () =>
                  context.actions.confirmHiddenApproval(
                    approval,
                    confirmation,
                  ),
              }}
            >
              {uiIcon(approvalIcon(confirmation.decision))}
              {confirmation.decision === "approve_always"
                ? "Confirm always allow tools"
                : "Confirm approve once"}
            </button>
          </div>
        </div>
      )}
      <div attrs={{ class: "form-actions approval-actions" }}>
        {approval.allowed_decisions.map((decision) => (
          <button
            key={JSON.stringify([
              approval.session_id,
              approval.approval_id,
              approval.tool_call_id,
              decision,
            ])}
            attrs={{
              type: "button",
              class: decision === "deny" ? "danger-button" : "primary-button",
              "data-autofocus":
                decision === "deny" && confirmation === null ? "true" : "false",
            }}
            props={{ disabled: !context.state.live }}
            on={{
              click: () => context.actions.decideApproval(approval, decision),
            }}
          >
            {uiIcon(approvalIcon(decision))}
            {approvalLabel(decision)}
          </button>
        ))}
      </div>
    </div>
  );
}

function questionField(
  context: PanelViewContext,
  prompt: QuestionPromptView,
  index: number,
  live: boolean,
): VNode {
  const draft = context.actions.questionDraft(prompt.question_id);
  return (
    <fieldset key={prompt.question_id} attrs={{ class: "question-fieldset" }}>
      <legend>{prompt.prompt}</legend>
      {prompt.options.map((option) => {
        const selected = draft.selected.has(option.option_id);
        return (
          <label key={option.option_id} attrs={{ class: "option-row" }}>
            <input
              attrs={{
                type: prompt.multiple ? "checkbox" : "radio",
                "aria-label": option.label,
              }}
              props={{ checked: selected, disabled: !live }}
              on={{
                change: () =>
                  context.actions.toggleQuestionOption(
                    prompt,
                    option.option_id,
                    selected,
                  ),
              }}
            />
            <span>
              <strong>{option.label}</strong>
              {option.description === null ? null : (
                <small>{option.description}</small>
              )}
            </span>
          </label>
        );
      })}
      {prompt.allows_other ? (
        <label attrs={{ class: "other-answer" }}>
          <span>{prompt.options.length === 0 ? "Your answer" : "Other"}</span>
          <textarea
            attrs={{
              rows: "4",
              maxlength: String(MAX_DRAFT_LENGTH),
              "data-autofocus":
                live && index === 0 && prompt.options.length === 0
                  ? "true"
                  : "false",
            }}
            props={{ value: draft.other, disabled: !live }}
            on={{
              input: (event: InputEvent) =>
                context.actions.updateQuestionOther(
                  prompt.question_id,
                  inputValue(event),
                ),
            }}
          ></textarea>
        </label>
      ) : null}
    </fieldset>
  );
}

function questionPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  const question = snapshot.pending_question;
  if (question === null) {
    return emptyPanel("This question is no longer pending.");
  }
  const answerEnabled = context.state.live && !question.content_truncated;
  return (
    <form
      attrs={{ class: "question-form" }}
      on={{
        submit: (event: SubmitEvent) => {
          event.preventDefault();
          context.actions.submitAnswers(question);
        },
      }}
    >
      {question.content_truncated ? (
        <p attrs={{ class: "truncation-note" }}>
          This question cannot be answered remotely because its exact content is unavailable.
          Cancel this request to continue, or review the original in the TUI.
        </p>
      ) : null}
      {question.questions.map((prompt, index) =>
        questionField(context, prompt, index, answerEnabled),
      )}
      <div attrs={{ class: "form-actions" }}>
        <button
          attrs={{ type: "submit", class: "primary-button" }}
          props={{
            disabled:
              !answerEnabled || !context.actions.answersComplete(question),
          }}
        >
          {uiIcon("send")}
          Submit answer
        </button>
        <button
          attrs={{ type: "button", class: "danger-button", "data-autofocus": question.content_truncated ? "true" : "false" }}
          props={{ disabled: !context.state.live || !snapshot.activity.cancellable || snapshot.activity.cancel_id === null }}
          on={{ click: context.actions.cancelQuestion }}
        >
          {uiIcon("stop")}
          Cancel request
        </button>
      </div>
    </form>
  );
}

function editorDiscardConfirmation(context: PanelViewContext): VNode {
  return (
    <div attrs={{ class: "confirmation-content" }}>
      <h3>Discard unsaved system prompt changes?</h3>
      <p>The local editor draft will be lost.</p>
      <div attrs={{ class: "form-actions" }}>
        <button
          attrs={{ type: "button", "data-autofocus": "true" }}
          on={{ click: context.actions.cancelEditorDiscard }}
        >
          {uiIcon("edit")}
          Keep editing
        </button>
        <button
          attrs={{ type: "button", class: "danger-button" }}
          on={{ click: () => context.actions.close(true) }}
        >
          {uiIcon("trash")}
          Discard
        </button>
      </div>
    </div>
  );
}

function confirmationPanel(
  context: PanelViewContext,
  snapshot: WebAppSnapshot,
): VNode {
  if (context.state.discardEditorConfirmation) {
    return editorDiscardConfirmation(context);
  }
  const data = panelData(snapshot, "confirmation");
  if (data === null) {
    return emptyPanel("No confirmation is pending.");
  }
  const confirmation = data.confirmation;
  const matches = context.actions.confirmationMatches(confirmation);
  return (
    <div attrs={{ class: "confirmation-content" }}>
      <h3>{confirmation.title}</h3>
      <p>{confirmation.message}</p>
      {!matches ? (
        <p attrs={{ class: "truncation-note" }}>
          This browser does not retain the exact initiating payload. Close this
          dialog and start the action again.
        </p>
      ) : null}
      <div attrs={{ class: "form-actions" }}>
        <button
          attrs={{ type: "button", "data-autofocus": "true" }}
          on={{ click: () => context.actions.close() }}
        >
          {uiIcon("close")}
          Cancel
        </button>
        <button
          attrs={{ type: "button", class: "danger-button" }}
          props={{ disabled: !context.state.live || !matches }}
          on={{ click: () => context.actions.confirm(confirmation) }}
        >
          {uiIcon("warning")}
          Confirm
        </button>
      </div>
    </div>
  );
}

const PANEL_RENDERERS = {
  command_palette: commandPalette,
  hotkeys: hotkeysPanel,
  themes: themesPanel,
  input_history: historyPanel,
  loop_detection: (context, snapshot) =>
    modesPanel(context, snapshot, "loop_modes"),
  system_prompt: systemPromptPanel,
  sessions: sessionsPanel,
  name_session: nameSessionPanel,
  latest_files: latestFilesPanel,
  models: modelsPanel,
  lsp_servers: lspPanel,
  agent_mode: (context, snapshot) =>
    modesPanel(context, snapshot, "agent_modes"),
  tool_approval: approvalPanel,
  ask_user: questionPanel,
  confirmation: confirmationPanel,
  skills: skillsPanel,
} as const satisfies Readonly<
  Record<PanelViewContext["state"]["panel"], PanelRenderer>
>;

function panelBody(context: PanelViewContext): VNode {
  const { snapshot, panel } = context.state;
  if (snapshot === null) {
    return emptyPanel("Waiting for synchronized state.");
  }
  return PANEL_RENDERERS[panel](context, snapshot);
}

export function renderPanel(context: PanelViewContext): VNode {
  const { panel, snapshot } = context.state;
  return (
    <div
      key={panelInstanceKey(panel, snapshot)}
      attrs={{ class: "overlay-backdrop", role: "presentation" }}
      on={{
        mousedown: (event: MouseEvent) => {
          if (event.target === event.currentTarget) {
            context.actions.close();
          }
        },
      }}
    >
      <section
        attrs={{
          class: `overlay-panel panel-${panel}`,
          role: "dialog",
          "aria-modal": "true",
          "aria-labelledby": "overlay-title",
          "data-focus-trap": "true",
        }}
      >
        <header attrs={{ class: "overlay-header" }}>
          <h2 attrs={{ id: "overlay-title" }}>{PANEL_TITLES[panel]}</h2>
          <button
            attrs={{
              type: "button",
              class: "icon-button",
              "aria-label": "Close overlay",
            }}
            on={{ click: () => context.actions.close() }}
          >
            {uiIcon("close")}
          </button>
        </header>
        <div attrs={{ class: "overlay-body" }}>{panelBody(context)}</div>
      </section>
    </div>
  );
}
