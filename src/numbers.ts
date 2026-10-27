/**
 * A number typed into a field; accepts a decimal comma and a trailing "mm". Null when it is not a
 * finite number. The fields are text, not type="number": a number field in an English-language
 * webview drops a typed comma, so "12,5" would read as 125.
 */
export function parseNumber(text: string): number | null {
  const t = text.trim().replace(/\s*mm$/i, "").replace(",", ".");
  if (t === "" || !/^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i.test(t)) return null;
  const v = Number(t);
  return Number.isFinite(v) ? v : null;
}
