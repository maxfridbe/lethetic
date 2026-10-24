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

await test("connection, prompt edits and Python toggles preserve the resumed session, history and files", async () => {
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
  transport.send({type:"save_system_prompt", name:"Browser prompt", content:"Be practical", session_id:id, confirmed_overwrite:false, confirmation_id:null},0);
  await Promise.resolve();
  assert.equal(new SessionStore(storage).list()[0]?.system, "Be practical");
  assert.deepEqual(new SessionStore(storage).list()[0]?.messages, messages);
  assert.ok(!transport.promptDetails.instructions.includes("Because you are in Python mode"));
  transport.setConnections([{...connection,apiKey:"replacement",thinking:"enabled"}],0);
  assert.equal(new SessionStore(storage).list()[0]?.connection?.thinking, "enabled");
  assert.equal(transport.sessionForExport(id).connection?.thinking, "enabled");
  transport.setPythonMode(true);
  assert.ok(transport.promptDetails.instructions.includes("Because you are in Python mode"));
  assert.equal(JSON.parse(transport.promptDetails.tools)[0].name, "python");
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

await test("duplicate endpoints resume the selected named connection and export without switching", () => {
  const values = new Map<string,string>();
  const storage = {getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}} as Storage;
  const original = {name:"Original",provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions",model:"test",apiKey:"first-key"};
  const copy = {...original,name:"Copy",apiKey:"second-key"};
  const make = () => new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},
    {connections:[original,copy],storage});
  let transport=make(); transport.start(); const id=transport.state.session.session_id;
  transport.setConnections([original,copy],1); transport.dispose();
  transport=make(); transport.start();
  assert.equal(transport.connection?.name,"Copy"); assert.equal(transport.connection?.apiKey,"second-key");
  assert.equal(transport.state.session.session_id,id);
  const exported=transport.sessionForExport(id); assert.equal(exported.connection?.name,"Copy");
  assert.ok(!JSON.stringify(exported).includes("second-key"));
  transport.openSession("new"); const current=transport.state.session.session_id;
  assert.equal(transport.sessionForExport(id).id,id); assert.equal(transport.state.session.session_id,current);
  transport.dispose();
});

await test(".NET mode and its network permission persist without changing session or shared files", async () => {
  const records = new Map<string, string>();
  const storage = { getItem: (key: string) => records.get(key) ?? null, setItem: (key: string, value: string) => { records.set(key, value); } } as Storage;
  const events = { socketOpened() {}, authenticationEpochChanged() {}, transportStatus() {}, protocolFault() {}, serverMessage() {}, commandResponse() {} };
  const files = new BrowserFiles();
  const transport = new LocalTransport(events, {connections:[],storage,files}); transport.start();
  const id = transport.state.session.session_id;
  files.save("uploaded.txt", "shared");
  transport.setMode("dotnet-wasm");
  assert.equal(transport.networkAccess, false);
  assert.match(transport.promptDetails.instructions, /Network access: disabled/);
  assert.equal(JSON.parse(transport.promptDetails.tools)[0].name, "dotnet");
  assert.equal(transport.siteAccess, false);
  transport.setSiteAccess(true);
  assert.match(transport.promptDetails.instructions, /Interactive sites: enabled/);
  transport.setNetworkAccess(true); transport.setNugetAccess(true); assert.equal(transport.state.session.session_id, id);
  assert.match(transport.promptDetails.instructions, /NuGet package access: enabled/);
  const saved = transport.sessionForExport(id);
  assert.equal(saved.siteAccess, true); assert.equal(saved.mode, "dotnet-wasm"); assert.equal(saved.networkAccess, true); assert.equal(saved.nugetAccess, true); assert.equal(saved.workspace?.files[0]?.path, "uploaded.txt");
  transport.dispose();
  const reopened = new LocalTransport(events, {connections:[],storage,initialSession:id}); reopened.start();
  assert.equal(reopened.siteAccess, true); assert.equal(reopened.mode, "dotnet-wasm"); assert.equal(reopened.networkAccess, true); assert.equal(reopened.nugetAccess, true);
  reopened.setPythonMode(true); assert.equal(reopened.sessionForExport(id).workspace?.files[0]?.path, "uploaded.txt");
  reopened.setMode("dotnet-wasm"); assert.equal(reopened.state.session.session_id, id);
  reopened.dispose();
});

await test("relay settings preserve the session and files, restore tab keys, and exclude keys from exports", () => {
  const values = new Map<string,string>(), keys = new Map<string,string>();
  const storage = {getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}} as Storage;
  const credentialStorage = {getItem:(key:string)=>keys.get(key)??null,setItem:(key:string,value:string)=>{keys.set(key,value);}} as Storage;
  const make = () => { const files = new BrowserFiles(storage); return {files, transport:new LocalTransport(
    {socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},
    {connections:[],storage,credentialStorage,files})}; };
  let {transport,files} = make(); transport.start(); const id = transport.state.session.session_id;
  transport.setMode("dotnet-wasm"); files.save("keep.txt", "42"); transport.setNetworkAccess(true);
  const relay = {endpoint:"https://relay.example/relay",token:"a-private-relay-token-1234567890"};
  transport.setDotnetRelay(relay);
  assert.equal(transport.state.session.session_id,id); assert.equal(files.list()[0]?.content,"42");
  assert.equal(transport.promptDetails.instructions.includes("host-configured HTTP relay"),true);
  assert.ok(!JSON.stringify([...values]).includes(relay.token));
  assert.ok(!JSON.stringify(transport.sessionForExport(id)).includes(relay.token));
  transport.dispose(); ({transport,files}=make()); transport.start();
  assert.deepEqual(transport.dotnetRelay,relay); assert.equal(transport.state.session.session_id,id);
  transport.openSession("new"); assert.equal(JSON.stringify(transport.dotnetRelay),"null");
  transport.openSession(id); assert.deepEqual(transport.dotnetRelay,relay);
  assert.throws(()=>transport.setDotnetRelay({endpoint:"socks5://localhost:9050"}),/SOCKS/);
  assert.throws(()=>transport.setDotnetRelay({endpoint:"https://bridges.torproject.org/"}),/Tor/);
  assert.deepEqual(transport.dotnetRelay,relay); assert.equal(transport.state.session.session_id,id);
  transport.setMode("python");
  assert.ok(transport.promptDetails.instructions.includes("Network access: enabled; Python"));
  assert.ok(transport.promptDetails.instructions.includes("host-configured HTTP relay"));
  transport.setNetworkAccess(false);
  assert.ok(transport.promptDetails.instructions.includes("Network access: disabled; use session files"));
  assert.equal(transport.state.session.session_id,id);assert.equal(files.list()[0]?.content,"42");
  transport.setDotnetRelay({...relay,token:""}); assert.equal(transport.dotnetRelay?.token,"");
  transport.dispose(); ({transport}=make()); transport.start(); assert.equal(transport.dotnetRelay?.token,""); transport.dispose();
});

await test("turn limits and shared loop controls persist per session without replacing files or history", async () => {
  const values=new Map<string,string>();let quota=false;
  const storage={getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{if(quota)throw Error("quota");values.set(key,value);}} as Storage;
  const make=()=>{
    const files=new BrowserFiles(storage);
    const transport=new LocalTransport({socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}},{connections:[],storage,files});
    transport.start();return {transport,files};
  };
  let {transport,files}=make();const id=transport.state.session.session_id;
  files.save("kept.txt","keep");
  const limits={timeoutMs:0,maxRounds:80,providerRetries:4,contextTokens:64000,maxOutputTokens:8192};
  transport.setTurnLimits(limits);
  transport.send({type:"set_loop_detection",session_id:id,mode_id:"off"},0);await Promise.resolve();
  assert.equal(transport.loopDetection,"off");
  quota=true;assert.throws(()=>transport.setTurnLimits({...limits,timeoutMs:60_000}),/quota/);quota=false;
  assert.deepEqual(transport.turnLimits,limits);assert.equal(transport.state.session.session_id,id);
  transport.dispose();({transport,files}=make());
  assert.equal(transport.state.session.session_id,id);assert.deepEqual(transport.turnLimits,limits);
  assert.equal(transport.loopDetection,"off");assert.equal(files.list()[0]?.content,"keep");
  transport.openSession("new");assert.equal(transport.turnLimits.timeoutMs,600_000);assert.equal(transport.loopDetection,"combined");
  transport.openSession(id);assert.deepEqual(transport.turnLimits,limits);assert.equal(transport.loopDetection,"off");
  transport.dispose();
});

await test("an in-flight checkpoint retains transcript and completed context without replaying the pending prompt", async () => {
  const values = new Map<string,string>();
  const storage = {getItem:(key:string)=>values.get(key)??null,setItem:(key:string,value:string)=>{values.set(key,value);}} as Storage;
  const connection = {name:"Recovery",provider:"openai" as const,endpoint:"https://example.com/v1/chat/completions",model:"test"};
  const events = {socketOpened(){},authenticationEpochChanged(){},transportStatus(){},protocolFault(){},serverMessage(){},commandResponse(){}};
  const transport = new LocalTransport(events,{connections:[connection],storage}); transport.start();
  const id=transport.state.session.session_id;
  transport.send({type:"send_prompt",session_id:id,prompt:"pending work"},0);
  await Promise.resolve();
  const saved=new SessionStore(storage).list().find(item=>item.id===id)!;
  assert.deepEqual(saved.messages,[]);
  assert.deepEqual(saved.inputs,["pending work"]);
  assert.ok(saved.blocks.some(item=>item.title==="Recovery checkpoint"));
  transport.dispose();
  await Promise.resolve();
  const resumed = new LocalTransport(events,{connections:[connection],storage});resumed.start();
  assert.equal(resumed.busy,false);
  assert.ok(resumed.state.blocks.blocks.some(item=>item.title==="Recovery checkpoint"));resumed.dispose();
});
