export const MAX_COMPARE = 4;

export function parseCompareIds(raw: unknown): string[] {
  const text = typeof raw === "string" ? raw : "";
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of text.split(/[,+\s]+/)) {
    const id = part.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    out.push(id);
    if (out.length >= MAX_COMPARE) break;
  }
  return out;
}
