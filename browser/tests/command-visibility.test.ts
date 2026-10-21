import assert from "node:assert/strict";
import test from "node:test";
import { browserCommandVisible, initialState } from "../src/local-state.js";
import { filteredCommands } from "../../web/src/app/helpers.js";
import type { LocalPaletteCommand } from "../../web/src/app/state.js";

await test("browser palettes hide unsupported commands and only offer Python reset in Python mode", () => {
  for (const mode of ["general", "python", "dotnet-wasm"]) {
    const snapshot = initialState();
    const visible = (command: { id: string }) => browserCommandVisible(command.id, mode);
    const commands = filteredCommands(snapshot, "", [], visible);
    assert.ok(commands.length < snapshot.commands.length);
    assert.ok(commands.every(command => command.disabled_reason !== "Requires the native runtime"));
    assert.equal(commands.some(command => command.id === "delete-python-runtime"), mode === "python");
    assert.ok(commands.some(command => command.id === "agent-mode"));
    assert.ok(commands.some(command => command.id === "python-permissive"));
    for (const unsupported of snapshot.commands.filter(command => command.disabled_reason === "Requires the native runtime")) {
      assert.ok(!filteredCommands(snapshot, unsupported.label, [], visible).some(command => command.id === unsupported.id));
    }
  }
});

await test("supported but temporarily disabled commands remain discoverable; native palettes are unchanged", () => {
  const snapshot = initialState();
  const local: LocalPaletteCommand = { id:"local:compact-session",label:"Compact session",description:"Keep this chat",icon:"command",behavior:"execute",accelerator:null,enabled:false,disabled_reason:"Finish the running turn",run() {} };
  const commands = filteredCommands(snapshot, "compact", [local], command => browserCommandVisible(command.id, "general"));
  assert.equal(commands[0], local);
  assert.equal(filteredCommands(snapshot, "").length, snapshot.commands.length);
});
