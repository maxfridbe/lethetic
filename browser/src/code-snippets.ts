import type { RenderBlockView } from "../../web/src/generated/contracts.js";

export interface CodeSnippet { code: string; language: string }

/** Extract complete fenced blocks and Python tool inputs, never tool output or prose. */
export function codeSnippets(block: RenderBlockView): CodeSnippet[] {
  if (block.tool?.kind === "call" && block.tool.tool_name === "python") {
    try {
      const args: unknown = JSON.parse(block.tool.payload);
      if (args && typeof args === "object" && "code" in args && typeof args.code === "string") {
        return [{ code: args.code, language: "python" }];
      }
    } catch { /* Incomplete tool JSON is not saveable code. */ }
    return [];
  }
  if (block.kind !== "markdown") return [];
  const snippets: CodeSnippet[] = [];
  let fence: string | null = null;
  let language = "";
  let lines: string[] = [];
  for (const line of block.content.split("\n")) {
    if (fence === null) {
      const match = /^ {0,3}(`{3,}|~{3,})([\w+-]*)\s*$/u.exec(line);
      if (match?.[1]) { fence = match[1]; language = match[2] ?? ""; lines = []; }
    } else if (new RegExp(`^ {0,3}${fence[0]}{${fence.length},}\\s*$`, "u").test(line)) {
      snippets.push({ code: lines.join("\n"), language: language || "text" });
      fence = null;
      if (snippets.length === 20) break;
    } else { lines.push(line); }
  }
  return snippets;
}
