import { PYTHON_TOOL_DEFINITION } from "./python.js";
import type { Tool } from "./sdk.js";

export const BROWSER_INSTRUCTIONS = `Browser environment
You are running in the Lethetic web frontend. Use only the tools supplied with this request. You have no native shell or access to the user's operating-system files. Session files are visible in the Files pane, where the user can preview supported formats, copy contents, and download individual files. Only claim that a file exists after a tool has created it successfully.`;

export const PYTHON_PROGRAM_EXAMPLE = String.raw`from pathlib import Path
program = Path("/home/pyodide/solution.py")
program.write_text("def main():\n    print(6 * 7)\n\nif __name__ == '__main__':\n    main()\n", encoding="utf-8")
import runpy
_ = runpy.run_path(str(program), run_name="__main__")`;

export const PYODIDE_CHECK = `import importlib.util
import sys
pyodide_available = importlib.util.find_spec("pyodide") is not None
running_in_pyodide = "pyodide" in sys.modules
print({"pyodide_available": pyodide_available, "running_in_pyodide": running_in_pyodide, "platform": sys.platform})`;

export const PYTHON_INSTRUCTIONS = `Python mode
Because you are in Python mode, the user expects you to write and run Python programs to solve their problems. Turn useful solutions into reusable .py files, execute them, check their output, and fix errors. Deliver the program as well as its results when that helps the task. Use clear filenames, small functions, and brief usage instructions.

Your Python tool runs real Python through Pyodide in a browser Web Worker. Python execution, session-file reads and writes, and browser-supported networking are already authorized. Proceed with the user's task without asking for per-call permission. The user can download programs and generated results directly from the web frontend's Files pane.

Save durable programs and outputs beneath /home/pyodide, the initial working directory. For example:
${PYTHON_PROGRAM_EXAMPLE}

Files in this workspace sync into the current session after each tool call. Mention the generated paths in your answer and tell the user to open Files to preview or download them. CSV, HTML, Markdown, SVG, PNG and other supported images can be previewed. HTML previews are static: scripts and external resources do not run. Save charts to files rather than relying on an interactive display. Python variables persist in the current worker but reset on reload, stop, or session switch; session files are restored. Do not put durable output elsewhere. Keep within 100 files, 256 folders, 1 MiB per file and 2 MiB total; symlinks are unsupported.

Pyodide availability and package checks
Use this check to distinguish an available module from the running Pyodide environment:
${PYODIDE_CHECK}

sys.platform == "emscripten" identifies an Emscripten Python build, not exclusively Pyodide. For another package, importlib.util.find_spec("package_name") checks whether it is currently importable, not whether it can be installed. The tool automatically loads packages bundled with Pyodide from imports in the submitted code. For additional compatible wheels, use import micropip followed by await micropip.install("package-name"). Native desktop binaries, subprocesses, and OS shell commands are unavailable. Network requests remain subject to browser CORS; use async browser-compatible APIs such as pyodide.http.pyfetch. Use print() to report useful output, and inspect exceptions before claiming success.`;

export function injectedInstructions(python: boolean): string {
  return [BROWSER_INSTRUCTIONS, ...(python ? [PYTHON_INSTRUCTIONS] : [])].join("\n\n");
}
export function effectiveSystemPrompt(editable: string, python: boolean): string {
  return [editable, injectedInstructions(python)].filter(Boolean).join("\n\n");
}
export function toolDefinitions(python: boolean, tools: readonly Tool[] = []): Pick<Tool, "name" | "description" | "parameters">[] {
  return [...tools, ...(python ? [PYTHON_TOOL_DEFINITION] : [])]
    .map(({ name, description, parameters }) => ({ name, description, parameters }));
}
