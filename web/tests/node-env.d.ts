// Minimal ambient declarations for the Node.js built-ins used by the web tests
// and the browser acceptance driver. The repository deliberately has no
// package manager and therefore no @types/node; declare only what is used.

interface NodeReadableStream {
  setEncoding(encoding: "utf8"): this;
  on(event: "data", listener: (chunk: string) => void): this;
}

interface NodeWritableStream {
  write(chunk: string): boolean;
}

interface NodeProcess {
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly stdin: NodeReadableStream & AsyncIterable<string>;
  readonly stdout: NodeWritableStream;
  readonly stderr: NodeWritableStream;
  exitCode: number | undefined;
}

declare var process: NodeProcess;

declare var Buffer: {
  from(data: string, encoding: "base64"): Uint8Array;
};

declare module "node:assert/strict" {
  type AssertMessage = string | Error;
  type ErrorMatcher = RegExp | object | ((error: unknown) => boolean);

  interface StrictAssert {
    ok(value: unknown, message?: AssertMessage): asserts value;
    equal<T>(actual: unknown, expected: T, message?: AssertMessage): asserts actual is T;
    notEqual(actual: unknown, expected: unknown, message?: AssertMessage): void;
    deepEqual<T>(actual: unknown, expected: T, message?: AssertMessage): asserts actual is T;
    match(value: string, regExp: RegExp, message?: AssertMessage): void;
    doesNotMatch(value: string, regExp: RegExp, message?: AssertMessage): void;
    rejects(
      block: Promise<unknown> | (() => Promise<unknown>),
      error?: ErrorMatcher,
      message?: AssertMessage,
    ): Promise<void>;
  }

  const assert: StrictAssert;
  export default assert;
}

declare module "node:test" {
  export default function test(
    name: string,
    fn: () => void | Promise<void>,
  ): Promise<void>;
}

declare module "node:fs/promises" {
  interface FileStats {
    isFile(): boolean;
  }

  export function readFile(path: string | URL, encoding: "utf8"): Promise<string>;
  export function writeFile(path: string | URL, data: string | Uint8Array): Promise<void>;
  export function stat(path: string | URL): Promise<FileStats>;
}

declare module "node:path" {
  export function resolve(...paths: string[]): string;
}

declare module "node:child_process" {
  type StdioOption = "ignore" | "pipe";

  interface SpawnOptions {
    readonly env?: Readonly<Record<string, string | undefined>>;
    readonly stdio?: readonly StdioOption[];
  }

  type NodePipe = NodeReadableStream & NodeWritableStream;

  export interface ChildProcess {
    readonly stderr: NodeReadableStream | null;
    readonly stdio: readonly (NodePipe | null | undefined)[];
    kill(signal?: "SIGTERM" | "SIGKILL"): boolean;
    on(event: "exit", listener: (code: number | null, signal: string | null) => void): this;
  }

  export function spawn(
    command: string,
    args: readonly string[],
    options?: SpawnOptions,
  ): ChildProcess;
}
