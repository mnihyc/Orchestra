/** Recorded checkpoints belong to the person's request and the answer of that same completed turn. */
import type { Item, Turn } from '../../src/generated/api-types.ts';
import { ADA } from './chat-rows.ts';
import { agentMessage, turn, userMessage } from './thread-rows.ts';

/** The same two-turn cold-opening fixture for the request budget and Recent navigation. */
export function recordedConversation(): { turns: Turn[]; items: Item[] } {
  const completed = '2026-10-15T10:00:01Z';
  return {
    turns: [
      { ...turn('recorded-one', { ordinal: 1, state: 'completed', inputId: 'input-one', completedAt: completed }), before_snapshot_id: 'copy-one' },
      { ...turn('recorded-two', { ordinal: 2, state: 'completed', inputId: 'input-two', completedAt: completed }), before_snapshot_id: 'copy-two' },
    ],
    items: [
      userMessage('request-one', 'Make the first change.', { author: ADA, inputId: 'input-one', turn: 'recorded-one' }),
      agentMessage('result-one', 'The first change is ready.', { turn: 'recorded-one' }),
      userMessage('request-two', 'Make the second change.', { author: ADA, inputId: 'input-two', turn: 'recorded-two' }),
      agentMessage('result-two', 'The second change is ready.', { turn: 'recorded-two' }),
    ],
  };
}
