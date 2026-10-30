/** Offset handling and batched streamed text, merged where the reader is behind.
 *
 * While an item is in progress its growing text arrives in pieces between the rows that carry the item.
 * Each piece says where in the text it begins and how long it is, so the reader keeps for every item
 * and field how far it has come (`applied`) and:
 *
 * - appends a piece that begins exactly there;
 * - ignores a piece that lies wholly behind (it came twice), and keeps only the new end of one that
 *   overlaps (the row it follows was ahead of it);
 * - treats a piece that begins beyond it as a gap: the item is read again and the text goes on from
 *   the length of that answer;
 * - lets a row that arrives replace the text only if the row is at least as long as what is held, so
 *   that a row that was written a moment before the last piece never takes text away.
 *
 * Text and summary are counted in Unicode code points, output in bytes of what the command printed;
 * only the tail of the output that is drawn (the last 16 KiB) is held.
 */
import type { Item } from '@/generated/api-types';

export type DeltaField = 'text' | 'summary' | 'output';

/** One piece of growing text, as the socket sends it. */
export interface Delta {
  item_id: string;
  field: DeltaField;
  offset: number;
  len: number;
  data: string;
}

/** How far each field of an item has come. */
export interface Applied {
  text?: number;
  summary?: number;
  output?: number;
}

/** How much of a command's output is held: the end of it, which is what is read while it runs. */
export const OUTPUT_TAIL_CHARS = 16 * 1024;

/** The most pieces held for an item that is not known yet or is being read again, before the oldest is dropped. */
export const HELD_PIECES_MOST = 256;

/** How many Unicode code points a text has. */
export function codePoints(text: string): number {
  let count = 0;
  for (let at = 0; at < text.length; at += 1) {
    const code = text.charCodeAt(at);
    // The first half of a surrogate pair and the second make one code point.
    if (code >= 0xd800 && code <= 0xdbff && at + 1 < text.length) at += 1;
    count += 1;
  }
  return count;
}

/** The text after its first `count` code points. */
export function dropCodePoints(text: string, count: number): string {
  let at = 0;
  for (let seen = 0; seen < count && at < text.length; seen += 1) {
    const code = text.charCodeAt(at);
    at += code >= 0xd800 && code <= 0xdbff && at + 1 < text.length ? 2 : 1;
  }
  return text.slice(at);
}

const encoder = new TextEncoder();

/** How far an item's own row says each of its fields has come. */
export function appliedOf(item: Item): Applied {
  switch (item.kind) {
    case 'agent_message':
      return { text: codePoints(item.payload.text) };
    case 'reasoning':
      return { summary: codePoints(item.payload.summary) };
    case 'tool_call':
      return item.payload.tool.kind === 'command' ? { output: item.payload.tool.output.total_len } : {};
    default:
      return {};
  }
}

/** Whether growing text of this field can belong to this item. */
export function growsBy(item: Item, field: DeltaField): boolean {
  return (
    (field === 'text' && item.kind === 'agent_message') ||
    (field === 'summary' && item.kind === 'reasoning') ||
    (field === 'output' && item.kind === 'tool_call' && item.payload.tool.kind === 'command')
  );
}

export type DeltaOutcome =
  /** The piece was added: the item as it now is, and how far the field has come. */
  | { kind: 'applied'; item: Item; applied: number }
  /** The piece lay behind what is held. */
  | { kind: 'behind' }
  /** The piece began beyond what is held: the item must be read again. */
  | { kind: 'gap' }
  /** The item is not one that grows by this field. */
  | { kind: 'foreign' };

/** The tail of an output that is held, with whether anything was cut from its front. */
function tail(text: string): { text: string; cut: boolean } {
  return text.length <= OUTPUT_TAIL_CHARS ? { text, cut: false } : { text: text.slice(text.length - OUTPUT_TAIL_CHARS), cut: true };
}

/** Adds one piece to an item whose field has come to `applied`. */
export function applyDelta(item: Item, applied: number, delta: Delta): DeltaOutcome {
  if (!growsBy(item, delta.field)) return { kind: 'foreign' };
  if (delta.offset > applied) return { kind: 'gap' };
  let data = delta.data;
  if (delta.offset < applied) {
    // Wholly behind, or overlapping: for text the new end can be told apart by code points; for bytes it cannot.
    if (delta.offset + delta.len <= applied || delta.field === 'output') return { kind: 'behind' };
    data = dropCodePoints(data, applied - delta.offset);
  }
  const reached = delta.offset + delta.len;
  if (item.kind === 'agent_message' && delta.field === 'text') {
    return { kind: 'applied', applied: reached, item: { ...item, payload: { ...item.payload, text: item.payload.text + data } } };
  }
  if (item.kind === 'reasoning' && delta.field === 'summary') {
    return { kind: 'applied', applied: reached, item: { ...item, payload: { ...item.payload, summary: item.payload.summary + data } } };
  }
  if (item.kind === 'tool_call' && item.payload.tool.kind === 'command' && delta.field === 'output') {
    const output = item.payload.tool.output;
    const kept = tail(output.inline + data);
    const next = {
      ...output,
      inline: kept.text,
      total_len: reached,
      truncated: output.truncated || kept.cut || reached > encoder.encode(kept.text).length,
    };
    return {
      kind: 'applied',
      applied: reached,
      item: { ...item, payload: { ...item.payload, tool: { ...item.payload.tool, output: next } } },
    };
  }
  return { kind: 'foreign' };
}

/**
 * Joins pieces that follow one another into one, for the same item and field: a reader that is
 * drawing once a frame needs one change a frame, whatever came in between.
 */
export function mergeDeltas(pieces: readonly Delta[]): Delta[] {
  const merged: Delta[] = [];
  for (const piece of pieces) {
    const last = merged.at(-1);
    if (last?.item_id === piece.item_id && last.field === piece.field && last.offset + last.len === piece.offset) {
      merged[merged.length - 1] = { ...last, len: last.len + piece.len, data: last.data + piece.data };
    } else {
      merged.push(piece);
    }
  }
  return merged;
}

/**
 * What a row that arrives makes of an item already held. The row is final when the item is no longer
 * in progress; before that it replaces the held text only when it is at least as long, so that a row
 * persisted a moment before the last piece cannot take the end of the text away.
 */
export function joinRow(held: Item, row: Item, applied: Applied): { item: Item; applied: Applied } {
  const fresh = appliedOf(row);
  if (row.status !== 'in_progress') return { item: row, applied: fresh };
  const behind = (field: keyof Applied): boolean => (fresh[field] ?? 0) < (applied[field] ?? 0);
  if (held.kind === 'agent_message' && row.kind === 'agent_message' && behind('text')) {
    return { item: { ...row, payload: { ...row.payload, text: held.payload.text } }, applied };
  }
  if (held.kind === 'reasoning' && row.kind === 'reasoning' && behind('summary')) {
    return { item: { ...row, payload: { ...row.payload, summary: held.payload.summary } }, applied };
  }
  if (held.kind === 'tool_call' && row.kind === 'tool_call' && behind('output') && held.payload.tool.kind === 'command' && row.payload.tool.kind === 'command') {
    return {
      item: { ...row, payload: { ...row.payload, tool: { ...row.payload.tool, output: held.payload.tool.output } } },
      applied,
    };
  }
  return { item: row, applied: fresh };
}
