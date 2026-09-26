import { MAX_ANSWER_BYTES, MAX_SYSTEM_PROMPT_BYTES, MAX_SYSTEM_PROMPT_NAME_BYTES, utf8Length, } from "./helpers.js";
export function getQuestionDraft(drafts, questionId) {
    const existing = drafts.get(questionId);
    if (existing !== undefined) {
        return existing;
    }
    const created = { selected: new Set(), other: "" };
    drafts.set(questionId, created);
    return created;
}
export function pruneQuestionDrafts(drafts, question) {
    const active = new Set(question?.questions.map((prompt) => prompt.question_id) ?? []);
    for (const id of drafts.keys()) {
        if (!active.has(id)) {
            drafts.delete(id);
        }
    }
}
export function questionAnswersComplete(question, drafts) {
    return question.questions.every((prompt) => {
        const draft = getQuestionDraft(drafts, prompt.question_id);
        return draft.selected.size > 0 || draft.other.trim().length > 0;
    });
}
export function questionAnswers(question, drafts) {
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
export function questionAnswersByteLength(answers) {
    return answers.reduce((total, answer) => total +
        utf8Length(answer.question_id) +
        answer.selected_option_ids.reduce((optionTotal, optionId) => optionTotal + utf8Length(optionId), 0) +
        (answer.other_text === null ? 0 : utf8Length(answer.other_text)), 0);
}
export function questionAnswersFit(answers) {
    return questionAnswersByteLength(answers) <= MAX_ANSWER_BYTES;
}
export function systemPromptSaveProblem(editorUnavailable, name, content) {
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
