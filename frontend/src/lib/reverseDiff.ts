/** A recorded text patch is reversed only at its exact lines, with no fuzzy search or discarded edits. */
import { parseDiff } from '@/lib/diff';

export function reverseDiff(current: string, diff: string): { text: string; diff: string } | null {
  const hunks = parseDiff(diff);
  if (hunks.length === 0 || !hunks.some((hunk) => hunk.lines.some((line) => line.kind !== 'context'))) return null;
  const lines = current.match(/[^\n]*\n|[^\n]+$/g) ?? [];
  const inverse: string[] = [];
  let end = 0;
  let oldEnd = 0;
  for (const hunk of hunks) {
    const old = hunk.lines.filter((line) => line.kind !== 'add');
    const next = hunk.lines.filter((line) => line.kind !== 'del');
    const start = hunk.newCount === 0 ? hunk.newStart : hunk.newStart - 1;
    const oldStart = hunk.oldCount === 0 ? hunk.oldStart : hunk.oldStart - 1;
    if (![hunk.oldStart, hunk.oldCount, hunk.newStart, hunk.newCount].every((number) => Number.isSafeInteger(number) && number >= 0)
      || old.length !== hunk.oldCount || next.length !== hunk.newCount || start < end || oldStart < oldEnd || start - end !== oldStart - oldEnd
      || start < 0 || oldStart < 0 || start + next.length > lines.length) return null;
    for (let at = 0; at < next.length; at++) {
      const line = next[at];
      if (line === undefined || lines[start + at] !== line.text + (line.noNewline === true ? '' : '\n')) return null;
    }
    end = start + next.length;
    oldEnd = oldStart + old.length;
    inverse.push(`@@ -${hunk.newStart},${hunk.newCount} +${hunk.oldStart},${hunk.oldCount} @@${hunk.heading === '' ? '' : ` ${hunk.heading}`}`);
    for (const line of hunk.lines) {
      inverse.push(`${line.kind === 'add' ? '-' : line.kind === 'del' ? '+' : ' '}${line.text}`);
      if (line.noNewline === true) inverse.push('\\ No newline at end of file');
    }
  }
  // Later coordinates are replaced first, preserving the earlier coordinates that were verified above.
  for (const hunk of [...hunks].reverse()) {
    const start = hunk.newCount === 0 ? hunk.newStart : hunk.newStart - 1;
    lines.splice(start, hunk.newCount, ...hunk.lines.filter((line) => line.kind !== 'add').map((line) => line.text + (line.noNewline === true ? '' : '\n')));
  }
  return { text: lines.join(''), diff: inverse.join('\n') + '\n' };
}
