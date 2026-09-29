import { isBoolean, isRecord, isString } from "./safety.js";
function exact(value, keys) {
    return isRecord(value) && Object.keys(value).length === keys.length &&
        keys.every((key) => Object.hasOwn(value, key));
}
export function isSkillChoiceView(value) {
    return exact(value, ["skill_id", "name", "description", "source", "enabled"]) &&
        isString(value["skill_id"]) && isString(value["name"]) && isString(value["description"]) &&
        isString(value["source"]) && isBoolean(value["enabled"]);
}
/** Catalog links may only point into Anthropic's skills repository. */
export const SKILL_REPOSITORY_PREFIX = "https://github.com/anthropics/skills/tree/";
export function isSkillCatalogView(value) {
    return exact(value, ["entry_id", "name", "summary", "url", "proprietary", "installed", "installing"]) &&
        isString(value["entry_id"]) && isString(value["name"]) && isString(value["summary"]) &&
        isString(value["url"]) && value["url"].startsWith(SKILL_REPOSITORY_PREFIX) &&
        isBoolean(value["proprietary"]) && isBoolean(value["installed"]) && isBoolean(value["installing"]);
}
