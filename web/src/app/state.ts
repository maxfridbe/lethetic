import type { VNode } from "../../lib/snabbdom/build/index.js";
import type {
  ApprovalDecision,
  CommandView,
  ConfirmationView,
  HistoryEntryView,
  PanelId,
  PendingApprovalView,
  PendingQuestionView,
  QuestionPromptView,
  UserAnswer,
  WebAppSnapshot,
  WebCommand,
} from "../generated/contracts.js";
import type { TransportStatus } from "../transport.js";
import type {
  AffirmativeApprovalDecision,
  ConfirmableCommand,
} from "./helpers.js";
import {
  MAX_ANSWER_BYTES,
  MAX_SYSTEM_PROMPT_BYTES,
  MAX_SYSTEM_PROMPT_NAME_BYTES,
  utf8Length,
} from "./helpers.js";

export interface PaletteState {
  query: string;
  selected: number;
}

export interface QuestionDraft {
  readonly selected: Set<string>;
  other: string;
}

export interface ApprovalConfirmation {
  readonly key: string;
  readonly decision: AffirmativeApprovalDecision;
}

export interface ChatViewState {
  readonly snapshot: WebAppSnapshot | null;
  readonly live: boolean;
  readonly activePanel: PanelId | null;
  readonly transportStatus: TransportStatus;
  readonly remoteReason: string | null;
  readonly pendingCount: number;
  readonly debuggerWide: boolean;
  readonly debuggerDrawerDismissed: boolean;
  readonly draft: string;
  readonly chatStart: number;
  readonly toast: string | null;
  readonly filesPane: VNode | null;
  readonly overlay: VNode | null;
}

export interface ChatViewActions {
  readonly openPalette: () => void;
  readonly invokeCommand: (command: CommandView) => void;
  readonly openPanel: (panel: "tool_approval" | "ask_user") => void;
  readonly updateDraft: (value: string) => void;
  readonly submitPrompt: () => void;
  readonly stop: () => void;
  readonly toggleDebugger: () => void;
  readonly onChatKeyDown: (event: KeyboardEvent) => void;
  readonly onChatManualIntent: (event: Event) => void;
  readonly onChatScroll: (event: Event) => void;
}

export interface ChatViewContext {
  readonly state: ChatViewState;
  readonly actions: ChatViewActions;
}

export interface PanelViewState {
  readonly panel: PanelId;
  readonly snapshot: WebAppSnapshot | null;
  readonly live: boolean;
  readonly palette: PaletteState | null;
  readonly editorName: string;
  readonly editorContent: string;
  readonly editorDirty: boolean;
  readonly sessionName: string;
  readonly discardEditorConfirmation: boolean;
  readonly approvalConfirmation: ApprovalConfirmation | null;
  readonly revision: number;
}

export interface PanelActions {
  readonly close: (forceDiscard?: boolean) => void;
  readonly invokeCommand: (command: CommandView) => void;
  readonly send: (command: WebCommand) => void;
  readonly sendConfirmable: (command: ConfirmableCommand) => void;
  readonly setPaletteQuery: (query: string) => void;
  readonly clampPaletteSelection: (selected: number) => void;
  readonly onPaletteKeyDown: (event: KeyboardEvent) => void;
  readonly selectHistoryEntry: (
    snapshot: WebAppSnapshot,
    entry: HistoryEntryView,
  ) => void;
  readonly selectSystemPrompt: (
    snapshot: WebAppSnapshot,
    promptId: string,
  ) => void;
  readonly submitSystemPrompt: (
    snapshot: WebAppSnapshot,
    editorUnavailable: boolean,
  ) => void;
  readonly updateEditorName: (name: string) => void;
  readonly updateEditorContent: (content: string) => void;
  readonly createSystemPrompt: () => void;
  readonly updateSessionName: (name: string) => void;
  readonly submitSessionName: (snapshot: WebAppSnapshot) => void;
  readonly cancelApprovalConfirmation: () => void;
  readonly decideApproval: (
    approval: PendingApprovalView,
    decision: ApprovalDecision,
  ) => void;
  readonly confirmHiddenApproval: (
    approval: PendingApprovalView,
    confirmation: ApprovalConfirmation,
  ) => void;
  readonly questionDraft: (questionId: string) => QuestionDraft;
  readonly toggleQuestionOption: (
    prompt: QuestionPromptView,
    optionId: string,
    wasSelected: boolean,
  ) => void;
  readonly updateQuestionOther: (questionId: string, value: string) => void;
  readonly answersComplete: (question: PendingQuestionView) => boolean;
  readonly submitAnswers: (question: PendingQuestionView) => void;
  readonly cancelQuestion: () => void;
  readonly cancelEditorDiscard: () => void;
  readonly confirmationMatches: (confirmation: ConfirmationView) => boolean;
  readonly confirm: (confirmation: ConfirmationView) => void;
}

export interface PanelViewContext {
  readonly state: PanelViewState;
  readonly actions: PanelActions;
}

export interface KeyboardState {
  readonly snapshot: WebAppSnapshot | null;
  readonly palette: PaletteState | null;
  readonly activePanel: PanelId | null;
  readonly debuggerDrawerOpen: boolean;
}

export interface KeyboardActions {
  readonly openPalette: () => void;
  readonly closeOverlay: () => void;
  readonly stop: () => void;
  /** First Escape during active work: tell the user a second one stops. */
  readonly armStop: () => void;
  readonly toggleDebugger: () => void;
  readonly requestQuit: () => void;
  readonly setPaletteSelection: (selected: number) => void;
  readonly invokeCommand: (command: CommandView) => void;
}

export interface KeyboardContext {
  readonly state: KeyboardState;
  readonly actions: KeyboardActions;
}

export function getQuestionDraft(
  drafts: Map<string, QuestionDraft>,
  questionId: string,
): QuestionDraft {
  const existing = drafts.get(questionId);
  if (existing !== undefined) {
    return existing;
  }
  const created: QuestionDraft = { selected: new Set(), other: "" };
  drafts.set(questionId, created);
  return created;
}

export function pruneQuestionDrafts(
  drafts: Map<string, QuestionDraft>,
  question: PendingQuestionView | null,
): void {
  const active = new Set(
    question?.questions.map((prompt) => prompt.question_id) ?? [],
  );
  for (const id of drafts.keys()) {
    if (!active.has(id)) {
      drafts.delete(id);
    }
  }
}

export function questionAnswersComplete(
  question: PendingQuestionView,
  drafts: Map<string, QuestionDraft>,
): boolean {
  return question.questions.every((prompt) => {
    const draft = getQuestionDraft(drafts, prompt.question_id);
    return draft.selected.size > 0 || draft.other.trim().length > 0;
  });
}

export function questionAnswers(
  question: PendingQuestionView,
  drafts: Map<string, QuestionDraft>,
): UserAnswer[] {
  return question.questions.map((prompt) => {
    const draft = getQuestionDraft(drafts, prompt.question_id);
    const other = draft.other.trim();
    return {
      question_id: prompt.question_id,
      selected_option_ids: Array.from(draft.selected),
      other_text: other.length === 0 ? null : other,
    };
  });
}

export function questionAnswersByteLength(answers: readonly UserAnswer[]): number {
  return answers.reduce(
    (total, answer) =>
      total +
      utf8Length(answer.question_id) +
      answer.selected_option_ids.reduce(
        (optionTotal, optionId) => optionTotal + utf8Length(optionId),
        0,
      ) +
      (answer.other_text === null ? 0 : utf8Length(answer.other_text)),
    0,
  );
}

export function questionAnswersFit(answers: readonly UserAnswer[]): boolean {
  return questionAnswersByteLength(answers) <= MAX_ANSWER_BYTES;
}

export function systemPromptSaveProblem(
  editorUnavailable: boolean,
  name: string,
  content: string,
): string | null {
  if (editorUnavailable) {
    return "Remote saving requires an active, exact system-prompt editor projection.";
  }
  if (name.length === 0) {
    return "Enter a prompt name before saving.";
  }
  if (utf8Length(name) > MAX_SYSTEM_PROMPT_NAME_BYTES) {
    return "Prompt name exceeds the 256-byte UTF-8 limit.";
  }
  if (content.length === 0) {
    return "Enter prompt content before saving.";
  }
  if (utf8Length(content) > MAX_SYSTEM_PROMPT_BYTES) {
    return "Prompt content exceeds the 256 KiB UTF-8 limit.";
  }
  return null;
}
