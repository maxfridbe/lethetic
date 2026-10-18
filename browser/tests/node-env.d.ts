declare module "node:fs/promises" {
  export function readFile(path: string | URL): Promise<Uint8Array<ArrayBuffer>>;
}
