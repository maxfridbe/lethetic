import { decodeBytes, parseWorkspace, type Workspace } from "./workspace.js";

/** ZIP STORE keeps the bounded session bytes intact, without loading a runtime. */
export function workspaceZip(value: Workspace): Uint8Array<ArrayBuffer> {
  const workspace = parseWorkspace(value);
  const chunks: Uint8Array<ArrayBuffer>[] = [], central: Uint8Array<ArrayBuffer>[] = [];
  let offset = 0;
  const encoder = new TextEncoder();
  const entries = [...workspace.directories.map(path => ({ path: path + "/", data: "" })), ...workspace.files];
  for (const file of entries) {
    const name = encoder.encode(file.path), data = decodeBytes(file.data);
    let crc = 0xffffffff;
    for (const byte of data) {
      crc ^= byte;
      for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
    crc = (crc ^ 0xffffffff) >>> 0;
    const header = new Uint8Array(30 + name.length), view = new DataView(header.buffer);
    view.setUint32(0, 0x04034b50, true); view.setUint16(4, 20, true); view.setUint16(6, 0x800, true);
    // Use a deterministic valid DOS date (1980-01-01) if no date is known.
    const modified = "modifiedAt" in file && typeof file.modifiedAt === "number" ? new Date(file.modifiedAt) : new Date(1980, 0, 1);
    const year = Math.min(2107, Math.max(1980, modified.getFullYear()));
    const date = ((year - 1980) << 9) | ((modified.getMonth() + 1) << 5) | modified.getDate();
    const time = (modified.getHours() << 11) | (modified.getMinutes() << 5) | (modified.getSeconds() >> 1);
    view.setUint16(10, time, true); view.setUint16(12, date, true);
    view.setUint32(14, crc, true); view.setUint32(18, data.length, true); view.setUint32(22, data.length, true);
    view.setUint16(26, name.length, true); header.set(name, 30);
    const record = new Uint8Array(46 + name.length), cv = new DataView(record.buffer);
    cv.setUint32(0, 0x02014b50, true); cv.setUint16(4, 20, true); record.set(header.subarray(4, 30), 6);
    cv.setUint32(38, file.path.endsWith("/") ? 0x10 : 0, true); cv.setUint32(42, offset, true); record.set(name, 46);
    chunks.push(header, data); central.push(record); offset += header.length + data.length;
  }
  const centralSize = central.reduce((sum, bytes) => sum + bytes.length, 0);
  const end = new Uint8Array(22), ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true); ev.setUint16(8, entries.length, true); ev.setUint16(10, entries.length, true);
  ev.setUint32(12, centralSize, true); ev.setUint32(16, offset, true);
  const result = new Uint8Array(offset + centralSize + end.length); let cursor = 0;
  for (const chunk of [...chunks, ...central, end]) { result.set(chunk, cursor); cursor += chunk.length; }
  return result;
}
