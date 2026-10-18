/** Exact byte counts can exceed Number's integer range in the native file protocol. */
export function fileSize(bytes: string | number | null): string {
  if (bytes === null || !/^\d+$/.test(String(bytes))) return "Unknown size";
  let value: bigint;
  try { value = BigInt(bytes); } catch { return "Unknown size"; }
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  let divisor = 1n, unit = 0;
  while (value >= divisor * 1024n && unit < units.length - 1) { divisor *= 1024n; unit++; }
  if (unit === 0) return `${value} B`;
  let tenths = (value * 10n + divisor / 2n) / divisor;
  if (tenths >= 10240n && unit < units.length - 1) { divisor *= 1024n; unit++; tenths = (value * 10n + divisor / 2n) / divisor; }
  return `${tenths / 10n}${tenths % 10n ? `.${tenths % 10n}` : ""} ${units[unit]}`;
}
