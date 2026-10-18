import assert from "node:assert/strict";
import test from "node:test";
import { LocalTransport } from "../src/local-transport.js";
import { commandResponseMatchesRequest, parseServerMessage } from "../../web/src/protocol.js";
import type { ICommandResponse, WebCommand } from "../../web/src/generated/contracts.js";

await test("local transport emits valid remote UI contracts and applies shared controls", async () => {
  let response: ICommandResponse | undefined;
  let snapshots = 0;
  const transport = new LocalTransport({
    socketOpened() {}, authenticationEpochChanged() {}, transportStatus() {}, protocolFault() {},
    serverMessage(message) {
      const parsed = parseServerMessage(JSON.stringify(message));
      assert.ok(parsed.ok, JSON.stringify(message));
      if (message.type === "state_snapshot") snapshots++;
    },
    commandResponse(value, request) {
      assert.ok(commandResponseMatchesRequest(value, request)); response = value;
    },
  }, { connections: [{ name: "Example", provider: "openai", endpoint: "https://example.com/v1/chat/completions", model: "test" }] });
  const send = async (command: WebCommand): Promise<ICommandResponse> => {
    response = undefined; transport.send(command, 0); await Promise.resolve();
    assert.ok(response); return response;
  };
  transport.start(); assert.equal(snapshots, 1);
  for (const command_id of ["themes", "models", "hotkeys", "input-history", "system-prompt", "name-session", "agent-mode"] as const) {
    assert.equal((await send({ type: "invoke_command", command_id })).result.status, "ok");
  }
  const theme = transport.state.themes.find((item) => !item.selected); assert.ok(theme);
  await send({ type: "select_theme", theme_id: theme.theme_id }); assert.equal(theme.selected, true);
  await send({ type: "invoke_command", command_id: "toggle-debugger" }); assert.equal(transport.state.debugger.open, true);
  const session_id = transport.state.session.session_id;
  const requested = await send({ type: "clear_context", session_id, confirmed: true, confirmation_id: null });
  assert.equal(requested.result.status, "error");
  const confirmation = transport.state.overlay.data; assert.equal(confirmation?.type, "confirmation");
  const confirmed = await send({ type: "clear_context", session_id, confirmed: true,
    confirmation_id: confirmation.confirmation.confirmation_id });
  assert.equal(confirmed.result.status, "ok"); assert.equal(transport.state.blocks.blocks.length, 0);
  await send({ type: "set_agent_mode", session_id, mode_id: "python" }); assert.equal(transport.pythonEnabled, true);
  await send({ type: "rename_session", session_id, name: "Local work" }); assert.equal(transport.state.session.display_name, "Local work");
  transport.dispose(); assert.equal(transport.send({ type: "request_snapshot" }, 0), null);
});
