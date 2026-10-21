import "../../web/tests/support/files-globals.js";
import assert from "node:assert/strict";
import test from "node:test";
import { BrowserFiles } from "../src/files.js";
import type { VNode } from "../../web/lib/snabbdom/build/index.js";
import { createFileUpload, readUploads } from "../src/file-upload.js";
import { emptyWorkspace, decodeBytes, encodeBytes, FILE_BYTES } from "../src/workspace.js";

await test("uploads preserve binary and Unicode data, number conflicts, and commit once", async () => {
  let workspace = emptyWorkspace(), writes = 0;
  const files = new BrowserFiles();
  files.bindWorkspace(() => workspace, next => { workspace = next; writes++; });
  workspace.directories = ["data", "data/folder.csv"];
  workspace.files = [{ path: "data/values.csv", data: encodeBytes(new TextEncoder().encode("original")) }];
  const incoming = await readUploads([new File(["héllo"], "values.csv"), new File([new Uint8Array([0,255,1])], "raw.bin"),
    new File([], "folder.csv"), new File(["again"], "values.csv")]);
  assert.deepEqual(files.importFiles("data", incoming), ["data/values (2).csv", "data/raw.bin", "data/folder (2).csv", "data/values (3).csv"]);
  assert.equal(writes, 1);
  assert.equal(new TextDecoder().decode(decodeBytes(files.get("data/values.csv")!.data)), "original");
  assert.equal(new TextDecoder().decode(decodeBytes(files.get("data/values (2).csv")!.data)), "héllo");
  assert.deepEqual([...decodeBytes(files.get("data/raw.bin")!.data)], [0,255,1]);
});

await test("invalid names and workspace quota failures leave the entire upload batch uncommitted", async () => {
  let workspace = emptyWorkspace(), writes = 0;
  const files = new BrowserFiles(); files.bindWorkspace(() => workspace, next => { workspace = next; writes++; });
  files.importFiles("", [{name:"keep.bin",bytes:new Uint8Array(FILE_BYTES)}]);
  const before = JSON.stringify(workspace);
  assert.throws(() => files.importFiles("", [{name:"ok",bytes:new Uint8Array()}, {name:"../bad",bytes:new Uint8Array()}]));
  assert.throws(() => files.importFiles("", [{name:"two",bytes:new Uint8Array(FILE_BYTES)}, {name:"three",bytes:new Uint8Array(FILE_BYTES)}, {name:"four",bytes:new Uint8Array(FILE_BYTES)}, {name:"five",bytes:new Uint8Array(1)}]), /64 MiB/);
  assert.equal(JSON.stringify(workspace), before); assert.equal(writes, 1);
  let read = false;
  const tooBig = new File([new Uint8Array(FILE_BYTES+1)], "large.bin");
  tooBig.arrayBuffer = async () => { read = true; return new ArrayBuffer(0); };
  await assert.rejects(readUploads([tooBig]), /16 MiB/); assert.equal(read, false);
});

await test("an upload cannot write into a different or busy session after asynchronous reading", async () => {
  for (const interruption of ["switch", "busy", "destroy"]) {
    let workspace = emptyWorkspace();
    const files = new BrowserFiles(); files.bindWorkspace(() => workspace, next => { workspace = next; });
    const session = { id: "original", busy: false };
    const messages: string[] = [];
    const upload = createFileUpload(files, () => session, message => messages.push(message));
    const view = upload.render("");
    const button = view.children![0] as VNode, input = view.children![1] as VNode;
    const click = button.data!.on!["click"]!, change = input.data!.on!["change"]!;
    assert.ok(typeof click === "function" && typeof change === "function");
    click.call(button, { currentTarget: { parentElement: { querySelector: () => ({ click() {} }) } } } as unknown as PointerEvent, button);
    const selected = new File(["content"], "late.txt");
    let release!: () => void;
    selected.arrayBuffer = () => new Promise(resolve => { release = () => resolve(new TextEncoder().encode("content").buffer); });
    change.call(input, { currentTarget: { files: [selected], value: "" } } as unknown as Event, input);
    if (interruption === "switch") session.id = "different";
    else if (interruption === "busy") session.busy = true;
    else upload.destroy();
    release(); await new Promise(resolve => setTimeout(resolve, 0));
    assert.deepEqual(workspace, emptyWorkspace());
    if (interruption !== "destroy") assert.ok(messages.some(message => message.includes("Upload canceled")));
    upload.destroy();
  }
});
