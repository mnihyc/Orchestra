/** Thread with item, view, file, line and document search parameters. Loader, permission handling and search schema are pending.
 *
 * The screen is the thread: its conversation with the agent and the composer. The item that an
 * address names (`item`) is brought into view and lit when the thread opens, with the others of a
 * selection it names (`items`).
 */
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { usePageParams, usePageSearch, usePageNavigate, usePagePush } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { useCallback, useState } from 'react';
import { selectThreadContinuation } from '@/api/threadRead';
import type { PromptFocus, SelectPromptVersion } from '@/features/timeline/context';

import { ThreadScreen } from '@/features/thread/Thread';
import { turnOrdinal } from '@/features/thread/turnLink';
import { CatchupRequest } from '@/features/inbox/away/Catchup';

interface ThreadSearch {
  item?: string;
  /** The items of a link to a selection, by id, separated by commas; `item` is the one brought into view. */
  items?: string;
  turn?: number;
  catchup?: boolean;
  continuation?: string;
  prompt?: string;
  /** The step of a sub-agent whose own conversation is shown in place of the main one. */
  agent?: string;
}

export const Route = createFileRoute('/thread/$threadId/')({
  staticData: { screen: 'thread' },
  validateSearch: (search: Record<string, unknown>): ThreadSearch => ({ ...(typeof search.item === 'string' && search.item !== '' ? { item:search.item } : {}),...(typeof search.items === 'string' && search.items !== '' ? { items:search.items } : {}),...(turnOrdinal(search.turn) === undefined ? {} : { turn:turnOrdinal(search.turn) }),...(search.catchup === true || search.catchup === 'true' ? { catchup:true } : {}), ...(typeof search.continuation === 'string' && search.continuation !== '' ? { continuation: search.continuation } : {}), ...(typeof search.prompt === 'string' && search.prompt !== '' ? { prompt: search.prompt } : {}), ...(typeof search.agent === 'string' && search.agent !== '' ? { agent: search.agent } : {}) }),
  component: ThreadRoute,
});

function ThreadRoute() {
  const { threadId } = usePageParams(Route);
  const read = useThreadReadRef(threadId);
  const { item, items, turn, catchup, continuation, prompt, agent } = usePageSearch(Route);
  const [focusPrompt, setFocusPrompt] = useState<PromptFocus | null>(null);
  const navigate = usePageNavigate(Route);
  const pushed = usePagePush(Route);
  const selectVersion = useCallback<SelectPromptVersion>((id, anchor) => {
    selectThreadContinuation(read, id);
    setFocusPrompt((held) => anchor?.focus === true ? { groupId: anchor.groupId, request: (held?.request ?? 0) + 1 } : null);
    void navigate({ search: id === null ? {} : { continuation: id,
      ...(anchor === undefined ? {} : { prompt: anchor.inputId, ...(anchor.itemId === null ? {} : { item: anchor.itemId }) }) } });
  }, [navigate, read]);
  // A sub-agent's conversation is part of the address, so that a link opens it; leaving it takes the parameter away. Going in and coming out each add an
  // entry to the history, so that the browser's Back returns from a sub-agent's conversation to the main one, and Forward to it again.
  const selectAgent = useCallback((to: string | null) => {
    void pushed({ search: (before) => { const next = { ...before }; if (to === null) delete next.agent; else next.agent = to; return next; }, push: true });
  }, [pushed]);
  return <><ThreadScreen key={threadId} threadId={threadId} item={item ?? null} items={items === undefined ? null : items.split(',').filter((one) => one !== '')} turn={turn} prompt={prompt} focusPrompt={focusPrompt} continuation={continuation ?? null} onVersionSelect={selectVersion} agent={agent ?? null} onAgent={selectAgent} />{catchup === true && <CatchupRequest threadId={threadId} open onOpenChange={(open) => { if (!open) void navigate({ search:(before) => { const next = { ...before }; delete next.catchup; return next; },replace:true }); }} />}</>;
}
