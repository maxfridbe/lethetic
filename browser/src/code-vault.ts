/** Passphrase-encrypted code at rest. Neither passphrases nor keys enter storage. */
export const CODE_VAULT_KEY = "lethetic.code-vault.v1";
const ITERATIONS = 600_000;
const MAX_BYTES = 1024 * 1024;
const encoder = new TextEncoder();
const AAD = encoder.encode("lethetic.code-vault.v1|PBKDF2-SHA256|600000|AES-256-GCM");

export interface CodeDocument { id: string; name: string; language: string; code: string; updatedAt: string }
interface Envelope { version: 1; iterations: number; salt: string; iv: string; ciphertext: string }
type VaultStorage = Pick<Storage, "getItem" | "setItem">;

export class CodeVault {
  #key: CryptoKey | null = null;
  #salt: Uint8Array<ArrayBuffer> | null = null;
  #documents: CodeDocument[] = [];
  #record: string | null = null;
  #generation = 0;
  #writing = false;

  constructor(readonly storage: VaultStorage) {}
  get unlocked(): boolean { return this.#key !== null; }
  get exists(): boolean { return this.storage.getItem(CODE_VAULT_KEY) !== null; }

  list(): CodeDocument[] {
    if (!this.unlocked) throw new Error("Unlock the code library first.");
    return this.#documents.map((document) => ({ ...document }));
  }

  lock(): void {
    this.#generation++;
    this.#key = null; this.#salt = null; this.#documents = []; this.#record = null;
  }

  async unlock(passphrase: string): Promise<void> {
    this.lock();
    const generation = this.#generation;
    const record = this.storage.getItem(CODE_VAULT_KEY);
    if (!passphrase || passphrase.length > 1024) throw new Error("Enter a passphrase of at most 1024 characters.");
    if (record === null && passphrase.length < 12) throw new Error("Use at least 12 characters for a new library passphrase.");
    const envelope = record === null ? null : parseEnvelope(record);
    const salt = envelope ? decode(envelope.salt, 16) : crypto.getRandomValues(new Uint8Array(16));
    const material = await crypto.subtle.importKey("raw", encoder.encode(passphrase), "PBKDF2", false, ["deriveKey"]);
    const key = await crypto.subtle.deriveKey({ name: "PBKDF2", hash: "SHA-256", iterations: ITERATIONS, salt },
      material, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
    let documents: CodeDocument[] = [];
    if (envelope) {
      try {
        const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: decode(envelope.iv, 12),
          additionalData: AAD, tagLength: 128 }, key, decode(envelope.ciphertext));
        documents = validateDocuments(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plain)));
      } catch { throw new Error("Unable to unlock. Check the passphrase; saved data may be damaged."); }
    }
    const initial = record ?? await encrypt(key, salt, documents);
    this.#checkUnchanged(generation, record);
    if (record === null) this.storage.setItem(CODE_VAULT_KEY, initial);
    this.#key = key; this.#salt = salt; this.#record = initial; this.#documents = documents;
  }

  async save(documents: CodeDocument[]): Promise<void> {
    if (!this.#key || !this.#salt) throw new Error("Unlock the code library first.");
    if (this.#writing) throw new Error("A save is already in progress.");
    const copy = validateDocuments(documents).map((document) => ({ ...document }));
    const generation = this.#generation;
    const previous = this.#record;
    this.#writing = true;
    try {
      const record = await encrypt(this.#key, this.#salt, copy);
      this.#checkUnchanged(generation, previous);
      this.storage.setItem(CODE_VAULT_KEY, record);
      this.#record = record; this.#documents = copy;
    } finally { this.#writing = false; }
  }

  #checkUnchanged(generation: number, previous: string | null): void {
    if (this.#generation !== generation) throw new Error("The library was locked before the operation finished.");
    if (this.storage.getItem(CODE_VAULT_KEY) !== previous) {
      throw new Error("The library changed in another tab. Lock and unlock it before saving again.");
    }
  }
}

async function encrypt(key: CryptoKey, salt: Uint8Array<ArrayBuffer>, documents: CodeDocument[]): Promise<string> {
  const plain = encoder.encode(JSON.stringify(documents));
  if (plain.byteLength > MAX_BYTES) throw new Error("The code library exceeds the 1 MiB limit.");
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: AAD, tagLength: 128 }, key, plain);
  return JSON.stringify({ version: 1, iterations: ITERATIONS, salt: encode(salt), iv: encode(iv),
    ciphertext: encode(new Uint8Array(ciphertext)) } satisfies Envelope);
}

function parseEnvelope(record: string): Envelope {
  if (record.length > 1_500_000) throw new Error("Saved code data is too large.");
  const value: unknown = JSON.parse(record);
  if (!value || typeof value !== "object") throw new Error("Invalid code library.");
  const item = value as Record<string, unknown>;
  if (item["version"] !== 1 || item["iterations"] !== ITERATIONS || typeof item["salt"] !== "string" ||
      typeof item["iv"] !== "string" || typeof item["ciphertext"] !== "string") {
    throw new Error("Unsupported or damaged code library.");
  }
  return item as unknown as Envelope;
}

function validateDocuments(value: unknown): CodeDocument[] {
  if (!Array.isArray(value) || value.length > 100) throw new Error("A library can contain at most 100 code files.");
  const seen = new Set<string>();
  for (const document of value) {
    if (!document || typeof document !== "object" ||
        typeof document.id !== "string" || !document.id || document.id.length > 64 || seen.has(document.id) ||
        typeof document.name !== "string" || !document.name.trim() || document.name.length > 200 ||
        /[\u0000-\u001f]/u.test(document.name) || typeof document.language !== "string" || document.language.length > 64 ||
        typeof document.code !== "string" || typeof document.updatedAt !== "string" ||
        document.updatedAt.length > 40 || !Number.isFinite(Date.parse(document.updatedAt))) {
      throw new Error("Invalid code file.");
    }
    seen.add(document.id);
  }
  return value as CodeDocument[];
}

function encode(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 32768) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768));
  }
  return btoa(binary);
}
function decode(text: string, length?: number): Uint8Array<ArrayBuffer> {
  const binary = atob(text);
  if (length !== undefined && binary.length !== length) throw new Error("Invalid encryption parameters.");
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
