/** Thread with item, view, file, line and document search parameters. Loader, permission handling and search schema are pending.
 *
 * The screen is the thread: its conversation with the agent and the composer. The item that an
 * address names (`item`) is brought into view and lit when the thread opens, with the others of a
 * selection it names (`items`).
 */
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
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
}

export const Route = createFileRoute('/thread/$threadId/')({
  staticData: { screen: 'thread' },
  validateSearch: (search: Record<string, unknown>): ThreadSearch => ({ ...(typeof search.item === 'string' && search.item !== '' ? { item:search.item } : {}),...(typeof search.items === 'string' && search.items !== '' ? { items:search.items } : {}),...(turnOrdinal(search.turn) === undefined ? {} : { turn:turnOrdinal(search.turn) }),...(search.catchup === true || search.catchup === 'true' ? { catchup:true } : {}), ...(typeof search.continuation === 'string' && search.continuation !== '' ? { continuation: search.continuation } : {}), ...(typeof search.prompt === 'string' && search.prompt !== '' ? { prompt: search.prompt } : {}) }),
  component: ThreadRoute,
});

function ThreadRoute() {
  const { threadId } = usePageParams(Route);
  const read = useThreadReadRef(threadId);
  const { item, items, turn, catchup, continuation, prompt } = usePageSearch(Route);
  const [focusPrompt, setFocusPrompt] = useState<PromptFocus | null>(null);
  const navigate = usePageNavigate(Route);
  const selectVersion = useCallback<SelectPromptVersion>((id, anchor) => {
    selectThreadContinuation(read, id);
    setFocusPrompt((held) => anchor?.focus === true ? { groupId: anchor.groupId, request: (held?.request ?? 0) + 1 } : null);
    void navigate({ search: id === null ? {} : { continuation: id,
      ...(anchor === undefined ? {} : { prompt: anchor.inputId, ...(anchor.itemId === null ? {} : { item: anchor.itemId }) }) } });
  }, [navigate, read]);
  return <><ThreadScreen key={threadId} threadId={threadId} item={item ?? null} items={items === undefined ? null : items.split(',').filter((one) => one !== '')} turn={turn} prompt={prompt} focusPrompt={focusPrompt} continuation={continuation ?? null} onVersionSelect={selectVersion} />{catchup === true && <CatchupRequest threadId={threadId} open onOpenChange={(open) => { if (!open) void navigate({ search:(before) => { const next = { ...before }; delete next.catchup; return next; },replace:true }); }} />}</>;
}
