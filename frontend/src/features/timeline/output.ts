/** Exact retained original intervals, shared by command and raw-call readers. */
import type { OutputRef } from '@/generated/api-types';

export function outputRetention(output: OutputRef) {
  const inlineBytes = new TextEncoder().encode(output.inline).length;
  const inlineStart = Math.max(0, output.total_len - inlineBytes);
  const range = output.retained;
  const known = output.blob !== null && range !== undefined && range !== null && range.offset >= 0 && range.len >= 0 && range.offset + range.len <= output.total_len;
  const missing: { from: number; to: number }[] = [];
  if (output.blob === null) {
    if (inlineStart > 0) missing.push({ from: 0, to: inlineStart });
  } else if (known) {
    const first = Math.min(range.offset, inlineStart);
    if (first > 0) missing.push({ from: 0, to: first });
    const end = range.offset + range.len;
    if (end < inlineStart) missing.push({ from: end, to: inlineStart });
  }
  return { inlineBytes, missing, unknown: output.blob !== null && !known,
    retainedBytes: output.blob === null ? inlineBytes : known ? range.len : null,
    full: known && range.offset === 0 && range.len === output.total_len };
}
