import assert from "node:assert/strict";
import test from "node:test";
import { relayEndpoint, relayToken, RelayKeys } from "../src/network-route.js";
await test("browser relay settings reject direct proxies, bridges and credentials in URLs", () => {
  for (const url of ["socks5://localhost:9050", "https://bridges.torproject.org/", "obfs4 127.0.0.1:80 fingerprint", "http://relay.example/relay", "https://key:secret@example.com/relay", "https://relay.example/relay?token=secret", "https://relay.example/relay#secret"])
    assert.throws(()=>relayEndpoint(url));
  assert.equal(relayEndpoint("http://127.0.0.1:5081/relay"),"http://127.0.0.1:5081/relay");
  assert.equal(relayToken(undefined),""); assert.throws(()=>relayToken("short"));
  assert.throws(()=>relayToken("long-token-with\nnewline-12345"));
});
await test("corrupt tab keys are ignored and failed storage writes do not claim success", () => {
  const bad = new RelayKeys({getItem:()=>"broken",setItem(){}}); assert.deepEqual(bad.values(),[]);
  const full = new RelayKeys({getItem:()=>null,setItem(){throw Error("quota");}});
  assert.throws(()=>full.set("https://example.com/relay","token"),/quota/); assert.equal(full.get("https://example.com/relay"),"");
});
