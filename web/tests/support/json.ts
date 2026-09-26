// Mutable JSON views for deliberately malformed wire fixtures.
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
export type JsonObject = { [key: string]: JsonValue };

function isJsonObject(value: JsonValue | undefined): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Deep-copy a typed fixture into a freely mutable JSON object. */
export function jsonClone(value: unknown): JsonObject {
  const copy: JsonValue = JSON.parse(JSON.stringify(value));
  if (!isJsonObject(copy)) {
    throw new Error("fixture is not a JSON object");
  }
  return copy;
}

/** Navigate object keys and array indices, requiring a JSON object at the end. */
export function objectAt(root: JsonValue, ...path: readonly (string | number)[]): JsonObject {
  let current: JsonValue | undefined = root;
  for (const segment of path) {
    if (typeof segment === "number") {
      current = Array.isArray(current) ? current[segment] : undefined;
    } else {
      current = isJsonObject(current) ? current[segment] : undefined;
    }
  }
  if (!isJsonObject(current)) {
    throw new Error(`fixture path ${path.join(".")} is not an object`);
  }
  return current;
}
