/** Bounded provider diagnostics, independent of model-history transactions. */
export const TRACE_LIMITS = { requests: 16, requestBytes: 128 * 1024, responseBytes: 1024 * 1024, sessionBytes: 8 * 1024 * 1024 } as const;
const encoder = new TextEncoder(), decoder = new TextDecoder();
const size = (text: string): number => encoder.encode(text).length;
function clip(text: string, limit: number, tail = false): string {
  const bytes = encoder.encode(text);
  if (bytes.length <= limit) return text;
  let index = tail ? bytes.length - limit : limit;
  if (tail) { while (index < bytes.length && (bytes[index]! & 0xc0) === 0x80) index++; return decoder.decode(bytes.subarray(index)); }
  while (index > 0 && (bytes[index]! & 0xc0) === 0x80) index--;
  return decoder.decode(bytes.subarray(0, index));
}
function variants(secrets: readonly string[]): string[] {
  return [...new Set(secrets.filter(Boolean).flatMap(secret => {
    const escaped = JSON.stringify(secret).slice(1,-1);
    // Tool arguments are themselves JSON strings inside the provider's JSON/SSE.
    return [secret, escaped, JSON.stringify(escaped).slice(1,-1), encodeURIComponent(secret)];
  }))].sort((a,b)=>b.length-a.length);
}
/** Hold enough lookahead to redact credentials even across network chunk boundaries. */
class Redactor {
  #pending = "";
  redacted = false;
  readonly #keep: number;
  constructor(private readonly secrets: readonly string[]) { this.#keep = Math.max(0, ...secrets.map(secret=>secret.length-1)); }
  push(text: string, final = false): string {
    if (!this.secrets.length) return text;
    this.#pending += text;
    const end = final ? this.#pending.length : Math.max(0, this.#pending.length-this.#keep);
    let output = "", position = 0;
    while (position < end) {
      const secret = this.secrets.find(secret=>this.#pending.startsWith(secret,position));
      if (secret) { output += "[REDACTED]"; position += secret.length; this.redacted = true; }
      else { const code = this.#pending.codePointAt(position)!; output += String.fromCodePoint(code); position += code > 0xffff ? 2 : 1; }
    }
    this.#pending = this.#pending.slice(position);
    return output;
  }
}
class Window {
  head = ""; tail = ""; total = 0;
  #headBytes = 0;
  #sealed = false;
  constructor(private readonly limit: number) {}
  push(text: string): void {
    this.total += size(text);
    const room = this.#sealed ? 0 : Math.max(0, Math.floor(this.limit/2)-this.#headBytes);
    const prefix = clip(text,room);
    this.head += prefix; this.#headBytes += size(prefix);
    if (prefix.length < text.length) this.#sealed = true;
    this.tail = clip(this.tail+text.slice(prefix.length),Math.floor(this.limit/2),true);
  }
  get omitted(): number { return this.total-size(this.head)-size(this.tail); }
}
export interface ProviderTrace {
  id: string; turnId: string; round: number; attempt: number; provider: string; model: string;
  startedAt: string; endedAt: string | null; outcome: "pending" | "complete" | "error" | "cancelled";
  httpStatus: number | null; contentType: string | null;
  requestBody: string; requestBytes: number; requestOmittedBytes: number;
  responseHead: string; responseTail: string; receivedBytes: number; responseOmittedBytes: number;
  responseComplete: boolean; redacted: boolean; error: string | null;
}
export interface ProviderTraceSnapshot { version: 1; omittedRequests: number; requests: ProviderTrace[] }
export class TraceCapture {
  readonly #redactor: Redactor;
  #response = new Window(TRACE_LIMITS.responseBytes);
  readonly #secrets: string[];
  readonly value: ProviderTrace;
  #finished = false;
  constructor(meta: Pick<ProviderTrace,"turnId"|"round"|"attempt"|"provider"|"model">, body: string, secrets: readonly string[]) {
    this.#secrets=variants(secrets); this.#redactor=new Redactor(this.#secrets);
    const request=new Redactor(this.#secrets), sanitized=request.push(body,true), captured=clip(sanitized,TRACE_LIMITS.requestBytes);
    const metadataRedactor = new Redactor(this.#secrets);
    this.value={...meta, provider:metadataRedactor.push(meta.provider,true), model:clip(metadataRedactor.push(meta.model,true),4096),id:crypto.randomUUID(),startedAt:new Date().toISOString(),endedAt:null,outcome:"pending",httpStatus:null,contentType:null,
      requestBody:captured,requestBytes:size(body),requestOmittedBytes:size(sanitized)-size(captured),
      responseHead:"",responseTail:"",receivedBytes:0,responseOmittedBytes:0,responseComplete:false,redacted:request.redacted || metadataRedactor.redacted,error:null};
  }
  response(status: number, contentType: string | null): void {
    const redactor = new Redactor(this.#secrets);
    this.value.httpStatus=status;
    if (status >= 400) this.#response = new Window(64 * 1024);
    this.value.contentType=contentType === null ? null : clip(redactor.push(contentType,true).split(';')[0]!.trim(),128);
    this.value.redacted ||= redactor.redacted;
  }
  chunk(text: string, bytes: number): void {
    if(this.#finished)return;
    this.value.receivedBytes+=bytes;this.#response.push(this.#redactor.push(text));this.#update();
  }
  received(): void { this.value.responseComplete=true; }
  finish(outcome: Exclude<ProviderTrace["outcome"],"pending">, error?: unknown): void {
    if(this.#finished)return;
    this.#finished=true;this.#response.push(this.#redactor.push("",true));this.#update();
    const redactor=new Redactor(this.#secrets);
    this.value.error=error === undefined ? null : clip(redactor.push(error instanceof Error ? error.message : String(error),true),4096);
    this.value.redacted ||= redactor.redacted;this.value.outcome=outcome;this.value.endedAt=new Date().toISOString();
  }
  #update():void {Object.assign(this.value,{responseHead:this.#response.head,responseTail:this.#response.tail,responseOmittedBytes:this.#response.omitted,redacted:this.value.redacted||this.#redactor.redacted});}
}
function traceSize(trace:ProviderTrace):number {return size(JSON.stringify(trace));}
export class ProviderTraceLog {
  #requests:ProviderTrace[]=[];
  #omitted=0;
  readonly #budget: number;
  constructor(budget:number=TRACE_LIMITS.sessionBytes) { this.#budget=Math.max(64*1024,Math.min(TRACE_LIMITS.sessionBytes,budget))-128; }
  add(trace:ProviderTrace):void {
    const index=this.#requests.findIndex(item=>item.id===trace.id);
    const copy=structuredClone(trace);
    if(index<0)this.#requests.push(copy);else this.#requests[index]=copy;
    while(this.#requests.length>1 && (this.#requests.length>TRACE_LIMITS.requests || this.#requests.reduce((sum,item)=>sum+traceSize(item),0)>this.#budget)){this.#requests.shift();this.#omitted++;}
    // Small embedding stores still retain the latest failure, with explicit truncation.
    const newest=this.#requests[0]!;
    while(traceSize(newest)>this.#budget && (newest.requestBody || newest.responseHead || newest.responseTail)) {
      const requestBytes=size(newest.requestBody), responseBytes=size(newest.responseHead)+size(newest.responseTail);
      newest.requestBody=clip(newest.requestBody,Math.floor(requestBytes/2));
      newest.responseHead=clip(newest.responseHead,Math.floor(size(newest.responseHead)/2));
      newest.responseTail=clip(newest.responseTail,Math.floor(size(newest.responseTail)/2),true);
      newest.requestOmittedBytes+=requestBytes-size(newest.requestBody);
      newest.responseOmittedBytes+=responseBytes-size(newest.responseHead)-size(newest.responseTail);
    }
  }
  snapshot():ProviderTraceSnapshot{return {version:1,omittedRequests:this.#omitted,requests:structuredClone(this.#requests)};}
  restore(value?:ProviderTraceSnapshot):void {
    const snapshot=value ? parseProviderTraces(value) : {version:1 as const,omittedRequests:0,requests:[]};
    this.#requests=[];this.#omitted=snapshot.omittedRequests;
    for(const trace of snapshot.requests)this.add(trace);
  }
}
export function parseProviderTraces(value:unknown):ProviderTraceSnapshot {
  const log=value as ProviderTraceSnapshot;
  if(!log || log.version!==1 || !Number.isSafeInteger(log.omittedRequests) || log.omittedRequests<0 || !Array.isArray(log.requests) || log.requests.length>TRACE_LIMITS.requests)throw Error("Invalid provider trace log.");
  let bytes=0;
  for(const trace of log.requests){
    if(!trace || !["pending","complete","error","cancelled"].includes(trace.outcome))throw Error("Invalid provider trace outcome.");
    for(const key of ["id","turnId","provider","model","startedAt","requestBody","responseHead","responseTail"] as const)if(typeof trace[key]!=="string")throw Error("Invalid provider trace text.");
    for(const key of ["endedAt","contentType","error"] as const)if(trace[key]!==null && typeof trace[key]!=="string")throw Error("Invalid provider trace metadata.");
    for(const key of ["round","attempt","requestBytes","requestOmittedBytes","receivedBytes","responseOmittedBytes"] as const)if(!Number.isSafeInteger(trace[key]) || trace[key]<0)throw Error("Invalid provider trace count.");
    if(trace.httpStatus!==null && (!Number.isSafeInteger(trace.httpStatus)||trace.httpStatus<100||trace.httpStatus>599))throw Error("Invalid provider trace status.");
    if(typeof trace.responseComplete!=="boolean"||typeof trace.redacted!=="boolean")throw Error("Invalid provider trace flags.");
    if(size(trace.requestBody)>TRACE_LIMITS.requestBytes || size(trace.responseHead)+size(trace.responseTail)>TRACE_LIMITS.responseBytes)throw Error("Provider trace exceeds capture limits.");
    for (const key of ["id","turnId","provider","model","startedAt","endedAt","contentType","error"] as const) {
      if (trace[key] !== null && size(trace[key]) > (key === "model" || key === "error" ? 4096 : 128)) throw Error("Provider trace metadata exceeds limits.");
    }
    bytes+=traceSize(trace);
  }
  if(bytes>TRACE_LIMITS.sessionBytes || new Set(log.requests.map(trace=>trace.id)).size!==log.requests.length)throw Error("Provider trace log exceeds limits or has duplicate requests.");
  return {version:1, omittedRequests:log.omittedRequests, requests:log.requests.map(trace => ({
    id:trace.id,turnId:trace.turnId,round:trace.round,attempt:trace.attempt,provider:trace.provider,model:trace.model,
    startedAt:trace.startedAt,endedAt:trace.endedAt,outcome:trace.outcome,httpStatus:trace.httpStatus,contentType:trace.contentType,
    requestBody:trace.requestBody,requestBytes:trace.requestBytes,requestOmittedBytes:trace.requestOmittedBytes,
    responseHead:trace.responseHead,responseTail:trace.responseTail,receivedBytes:trace.receivedBytes,responseOmittedBytes:trace.responseOmittedBytes,
    responseComplete:trace.responseComplete,redacted:trace.redacted,error:trace.error,
  }))};
}

/** Remove known credentials before displaying provider-controlled error text. */
export function redactProviderText(text: string, secrets: readonly string[]): string {
  return clip(new Redactor(variants(secrets)).push(text, true), 4096);
}
