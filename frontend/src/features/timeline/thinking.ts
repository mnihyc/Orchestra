/** What the agent is thinking about now, in one line: the first line of the summary of the thought that is being written.
 *
 * Both programs give a summary of their thinking, never the thinking itself. A reasoning item is one step of the work of its turn; while it is
 * being written, the status line names it by this line. It is read from the thread as the page already holds it: `useThreadPart(threadId, thinkingLine)`.
 */
import { runningTurn, type ThreadView } from '@/realtime/stores/thread';

/** The first line of a summary that says something, without the marks of Markdown around its words. */
export function firstSummaryLine(summary: string): string | null {
  for (const line of summary.split(/\r?\n/)) {
    const text = line
      .replace(/^\s{0,3}(?:#{1,6}\s+|[-*+]\s+|\d+[.)]\s+|>\s*)/, '')
      .replace(/\*\*(.+?)\*\*/g, '$1')
      .replace(/__(.+?)__/g, '$1')
      .replace(/`([^`]*)`/g, '$1')
      .trim();
    if (text !== '') return text;
  }
  return null;
}

/** The first line of the summary of the thought being written in the turn that runs, or null when none is. */
export function thinkingLine(view: ThreadView): string | null {
  const turn = runningTurn(view);
  if (turn === null) return null;
  for (let at = view.order.length - 1; at >= 0; at -= 1) {
    const item = view.items.get(view.order[at] ?? '');
    if (item?.turn_id !== turn.id) continue;
    // The thought that is being written is the newest thing the turn made: what the agent does after thinking ends it.
    if (item.kind === 'reasoning') return item.status === 'in_progress' ? firstSummaryLine(item.payload.summary) : null;
    if (item.kind === 'tool_call' || item.kind === 'agent_message') return null;
  }
  return null;
}
