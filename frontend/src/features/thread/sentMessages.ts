/** Which of the messages that wait stand in the conversation as the person's own message, and which stay in the queue.
 *
 * A message sent to a thread that takes it into a turn of its own (it is idle, or it starts, or the turn for it is still
 * going through its start steps) is a message at once, dimmed until the agent takes it. The queue above the composer holds only what waits behind a turn that runs, and whatever
 * cannot go on by itself: a held input, one whose files hold it, a line that is not a person's, a send that failed.
 */
import type { SendingInput } from '@/features/composer/Draft';
import { queuedInputs, sendingInTray } from '@/features/timeline/rows';
import type { ContentPart, ThreadInput } from '@/generated/api-types';
import { runningTurn, type ThreadView } from '@/realtime/stores/thread';

/** A message that is on its way to the agent, drawn in the conversation. */
export interface SentMessage {
  /** The same from the moment it was sent until the agent took it. */
  key: string;
  content: readonly ContentPart[];
  at: string;
  /** The accepted waiting input, whose existing correction and removal acts remain available. */
  input: ThreadInput | null;
}

export interface WaitingMessages {
  message: SentMessage | null;
  /** What stays in the queue. */
  queued: readonly ThreadInput[];
  sending: readonly SendingInput[];
}

/** The input a turn that is still starting takes: until its agent has it, that turn's message is on its way. */
function startingInput(view: ThreadView): string | null {
  const turn = runningTurn(view);
  return turn?.state === 'starting' ? turn.input_id : null;
}

/** A turn of its own is next: no turn runs, and none waits for a person. */
function takesNext(view: ThreadView): boolean {
  const state = view.thread?.state;
  return state === 'idle' || state === 'starting';
}

/** What an input that is being made ready for its turn says of itself: its files are being written, or were. It is not held. */
const MADE_READY = new Set(['files', 'files_ready']);

/** Whether an accepted input can go on by itself, as the person's message. */
function standsAsMessage(input: ThreadInput): boolean {
  return input.source === 'user' && !input.queue_held && (input.state_detail === null || MADE_READY.has(input.state_detail)) && input.attachment_issues.length === 0;
}

export function partitionWaiting(view: ThreadView, sending: readonly SendingInput[]): WaitingMessages {
  const queued = queuedInputs(view);
  const tray = sendingInTray(view, sending);
  const message = (input: ThreadInput): SentMessage => ({ key: input.client_id ?? input.id, content: input.content, at: input.created_at, input });
  // A turn going through its start steps takes its own message, wherever that stands in the queue, and whatever the thread says of itself.
  const taken = startingInput(view);
  const starting = taken === null ? undefined : queued.find((input) => input.id === taken);
  if (starting !== undefined) return { message: message(starting), queued: queued.filter((input) => input !== starting), sending: tray };
  // Where its message already stands in the conversation, what else waits is behind that turn.
  if (taken !== null || !takesNext(view)) return { message: null, queued, sending: tray };
  const first = queued[0];
  if (first !== undefined) {
    if (!standsAsMessage(first)) return { message: null, queued, sending: tray };
    return { message: message(first), queued: queued.slice(1), sending: tray };
  }
  const own = tray[0];
  if (own?.state !== 'sending') return { message: null, queued, sending: tray };
  return { message: { key: own.clientId, content: own.content, at: own.at, input: null }, queued, sending: tray.slice(1) };
}
