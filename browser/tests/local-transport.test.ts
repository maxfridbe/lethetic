import assert from "node:assert/strict";
import test from "node:test";
import { BrowserFiles } from "../src/files.js";
import { SessionStore } from "../src/sessions.js";
import { textFile } from "../src/workspace.js";
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
  const pythonSession = transport.state.session.session_id;
  await send({ type: "rename_session", session_id: pythonSession, name: "Local work" }); assert.equal(transport.state.session.display_name, "Local work");
  await send({ type: "new_session" });
  const freshSession = transport.state.session.session_id;
  assert.notEqual(freshSession, pythonSession);
  assert.ok(transport.state.sessions.sessions.some((session) => session.session_id === pythonSession && session.display_name === "Local work"));
  await send({ type: "resume_session", session_id: pythonSession });
  assert.equal(transport.state.session.display_name, "Local work");
  assert.equal(transport.pythonEnabled, true);
  await send({ type: "delete_session", session_id: freshSession, confirmed: false, confirmation_id: null });
  const deletion = transport.state.overlay.data; assert.equal(deletion?.type, "confirmation");
  await send({ type: "delete_session", session_id: freshSession, confirmed: true, confirmation_id: deletion.confirmation.confirmation_id });
  assert.ok(!transport.state.sessions.sessions.some((session) => session.session_id === freshSession));
  assert.equal(transport.state.session.session_id, pythonSession);
  await send({ type: "wipe_sessions", confirmed: false, confirmation_id: null });
  const wipe = transport.state.overlay.data; assert.equal(wipe?.type, "confirmation");
  await send({ type: "wipe_sessions", confirmed: true, confirmation_id: wipe.confirmation.confirmation_id });
  assert.equal(transport.state.sessions.sessions.length, 1);
  assert.notEqual(transport.state.session.session_id, pythonSession);
  transport.dispose(); assert.equal(transport.send({ type: "request_snapshot" }, 0), null);
});

await test("Files are session-scoped, reopen with their session and preserve old files on quota failure", async () => {
  const values = new Map<string,string>(); let quota = false;
  const storage = { getItem:(key:string)=>values.get(key)??null, setItem:(key:string,value:string)=>{if(quota)throw Error('quota');values.set(key,value);} } as Storage;
  const make = () => {
    const files = new BrowserFiles(storage);
    const transport = new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},
      {connections:[],storage,files});
    transport.start(); return {files,transport};
  };
  let {files,transport}=make(); const first=transport.state.session.session_id;
  files.save('input.csv','first session');
  transport.openSession('new'); const second=transport.state.session.session_id;
  assert.deepEqual(files.list(),[]); files.save('input.csv','second session');
  transport.openSession(first); assert.equal(files.list()[0]?.content,'first session');
  quota=true; assert.throws(()=>files.save('input.csv','lost'),/quota/);
  assert.equal(files.list()[0]?.content,'first session'); quota=false;
  transport.dispose(); ({files,transport}=make());
  assert.equal(transport.state.session.session_id,first); assert.equal(files.list()[0]?.content,'first session');
  transport.openSession(second); assert.equal(files.list()[0]?.content,'second session');
  files.remove('input.csv'); transport.dispose(); ({files,transport}=make());
  assert.deepEqual(files.list(),[]);
  const saved = new SessionStore(storage).list().find(session=>session.id===first);
  assert.equal(textFile(saved!.workspace!.files[0]!), 'first session'); transport.dispose();
});

await test("connection edits and Python toggles preserve the resumed session, history and files", () => {
  const values = new Map<string,string>();
  const storage = {getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}} as Storage;
  const connection = {name:"Saved",provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions",model:"test"};
  const id = crypto.randomUUID();
  const messages = [{role:"user",content:"remember me"},{role:"assistant",content:"remembered"}];
  new SessionStore(storage).save({id,name:"Work",label:"remember me",updatedAt:new Date().toISOString(),
    blocks:[],messages,inputs:["remember me"],system:"help",python:false,theme:"dracula",connection});
  const files = new BrowserFiles(storage);
  const transport = new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},
    {connections:[connection],storage,files});
  transport.start(); files.save("keep.csv","42");
  transport.setConnections([{...connection,apiKey:"replacement"}],0);
  transport.setPythonMode(true);
  assert.equal(transport.state.session.session_id,id);
  assert.equal(transport.state.session.display_name,"Work");
  assert.equal(files.list()[0]?.content,"42");
  let saved = new SessionStore(storage).list();
  assert.equal(saved.length,1); assert.equal(saved[0]?.python,true); assert.deepEqual(saved[0]?.messages,messages);
  transport.setConnections([{...connection,model:"other",apiKey:"replacement"}],0);
  transport.setPythonMode(false);
  assert.equal(transport.state.session.session_id,id);
  saved = new SessionStore(storage).list();
  assert.equal(saved.length,1); assert.equal(saved[0]?.python,false); assert.deepEqual(saved[0]?.messages,messages);
  assert.ok(!JSON.stringify([...values]).includes("replacement"));
  transport.dispose();
});
