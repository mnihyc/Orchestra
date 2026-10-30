/** A search result opens in its own workbench tab beside the current conversation and returns to its opener. */
import { chatAddress } from '@/features/chat/targets';
import { openPage } from '@/features/workbench/PageView';
import { useRouter } from '@tanstack/react-router';
import { ArrowLeft, ExternalLink, Search } from 'lucide-react';
import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { client } from '@/api/client';
import { pathOf, type Destination } from '@/features/finder/destinations';
import { captureFocus, restoreFocus, type ReturnFocus } from '@/features/search/returnFocus';
import { activate, allTabs, close, open, setTabTarget, type AreaId, type BarId, type Layout } from '@/features/workbench/layout';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useWorkbench } from '@/features/workbench/store';
import { registerView, type ViewProps } from '@/features/workbench/views';
import type { SearchHit } from '@/generated/api-types';
import type { PromptFocus } from '@/features/timeline/context';
import { writeViewChoice } from '@/lib/viewChoices';
import { Button } from '@/ui/Button';
import { SkeletonRows } from '@/ui/Skeleton';

const NAME = 'search-result';
const Chat = lazy(() => import('@/features/chat/ChatView').then((module) => ({ default: module.ChatView })));
const Document = lazy(() => import('@/features/search/ResultContent').then((module) => ({ default: module.ResultDocument })));
const Thread = lazy(() => import('@/features/search/ResultContent').then((module) => ({ default: module.ResultThread })));
const PlaceChat = lazy(() => import('@/features/search/ResultContent').then((module) => ({ default: module.ResultPlaceChat })));

export interface ResultTarget {
  kind: 'thread' | 'document' | 'chat' | 'place-chat';
  id: string;
  anchor?: string;
  continuationId?: string | null;
  promptId?: string;
}

/** The actual content and exact place a hit names; a node's chat remains a chat. */
export function targetOfHit(hit: SearchHit): ResultTarget | null {
  switch (hit.kind) {
    case 'name': return hit.node.kind === 'thread' ? { kind: 'thread', id: hit.node.id, continuationId: null } : null;
    case 'item': return { kind: 'thread', id: hit.thread.id, anchor: hit.item.id, continuationId: hit.projection?.viewed_continuation_id ?? null };
    case 'chat_message': return { kind: 'chat', id: hit.chat.id, anchor: hit.message.id };
    case 'document': return { kind: 'document', id: hit.document.id, ...(hit.block_id === null ? {} : { anchor: hit.block_id }) };
  }
}

/** Content a finder destination can open beside the route, using its ordinary target and anchor. */
export function targetOfDestination(destination: Destination): ResultTarget | null {
  // Read the canonical address for names and the filled route for commands through the same parser.
  const address = new URL(pathOf(destination), 'https://orch.invalid');
  const chat = address.searchParams.get('chat');
  const message = address.searchParams.get('message');
  if (chat !== null) return { kind: 'chat', id: chat, ...(message === null ? {} : { anchor: message }) };
  const place = /^\/(org|project|topic|thread|doc)\/([^/]+)$/.exec(address.pathname);
  if (place?.[2] === undefined) return null;
  const id = decodeURIComponent(place[2]);
  if (address.searchParams.get('panel') === 'chat') return { kind: 'place-chat', id };
  const anchor = address.searchParams.get(place[1] === 'thread' ? 'item' : 'block');
  const prompt = address.searchParams.get('prompt');
  const continuation = address.searchParams.get('continuation');
  if (place[1] === 'thread') return { kind: 'thread', id, continuationId: continuation === '' ? null : continuation,
    ...(prompt === null || prompt === '' ? {} : { promptId: prompt }), ...(anchor === null ? {} : { anchor }) };
  if (place[1] === 'doc') return { kind: 'document', id, ...(anchor === null ? {} : { anchor }) };
  return null;
}

const openers = new Map<string, ReturnFocus>();

/** Branch intent is part of a thread result's identity, even when no href was supplied. */
function sameResult(one: Record<string, string> | undefined, other: Record<string, string>): boolean {
  if (one === undefined || one.kind !== other.kind) return false;
  if (other.kind !== 'thread') return Object.entries(other).every(([key, value]) => one[key] === value);
  return one.id === other.id && (one.anchor ?? '') === (other.anchor ?? '') && (one.prompt ?? '') === (other.prompt ?? '')
    && (one.continuation ?? '') === (other.continuation ?? '');
}

export function threadResultHref(of: Record<string, string>): string {
  const url = new URL(`/thread/${encodeURIComponent(of.id ?? '')}`, 'https://orch.invalid');
  if (of.anchor !== undefined && of.anchor !== '') url.searchParams.set('item', of.anchor);
  if (of.continuation !== undefined && of.continuation !== '') url.searchParams.set('continuation', of.continuation);
  if (of.prompt !== undefined && of.prompt !== '') url.searchParams.set('prompt', of.prompt);
  return url.pathname + url.search;
}

/** Reuses an exact preview in an explicitly requested sidebar or bottom area. */
export function resultLayout(layout: Layout, of: Record<string, string>, area: BarId): Layout {
  const existing = allTabs(layout).find((tab) => tab.view === NAME && sameResult(tab.of, of));
  if (existing !== undefined) return activate(layout, existing.id);
  return open(layout, NAME, area, of);
}

/** Opens authorized content through its normal reader, keeping the current route and draft. */
export function openResult(target: ResultTarget, destination: Destination | null, area: AreaId = 'main'): void {
  const focused = captureFocus();
  const of: Record<string, string> = { kind: target.kind, id: target.id };
  if (destination !== null) of.href = pathOf(destination);
  if (target.anchor !== undefined) of.anchor = target.anchor;
  if (target.kind === 'thread') of.continuation = target.continuationId ?? '';
  if (target.promptId !== undefined) of.prompt = target.promptId;
  if (area === 'main') {
    const href = target.kind === 'thread' ? threadResultHref(of)
      : target.kind === 'document' ? `/doc/${encodeURIComponent(target.id)}${target.anchor === undefined ? '' : `?block=${encodeURIComponent(target.anchor)}`}`
        : target.kind === 'chat' ? chatAddress(target.id, target.anchor) : `/place-chat/${encodeURIComponent(target.id)}`;
    openPage(href, 'beside'); return;
  }
  const workbench = useWorkbench.getState();
  workbench.change((layout) => resultLayout(layout, of, area));
  const tab = allTabs(useWorkbench.getState().layout).find((candidate) => candidate.view === NAME && sameResult(candidate.of, of));
  if (tab === undefined) return;
  openers.set(tab.id, focused);
  const user = client.session.getState().userId;
  writeViewChoice(user, 'global', 'mobileTab', tab.id);
  writeViewChoice(user, 'global', 'mobileView', NAME);
  requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-search-result="${CSS.escape(tab.id)}"]`)?.focus());
}

function restoreOpener(tabId: string): void {
  const opener = openers.get(tabId);
  openers.delete(tabId);
  queueMicrotask(() => restoreFocus(opener));
}

function ResultView({ tab }: ViewProps) {
  const { t } = useTranslation();
  const router = useRouter();
  const shown = useViewShown();
  const root = useRef<HTMLElement>(null);
  const [focusPrompt, setFocusPrompt] = useState<PromptFocus | null>(null);
  const of = tab.of;
  const href = of?.kind === 'thread' ? threadResultHref(of) : of?.href;
  useEffect(() => () => { openers.delete(tab.id); }, [tab.id]);
  if (of?.id === undefined) return null;
  const back = () => {
    // Restore before removing the tab: its unmount also releases any retained opener.
    restoreOpener(tab.id);
    useWorkbench.getState().change((layout) => close(layout, tab.id));
    writeViewChoice(client.session.getState().userId, 'global', 'mobileView', '');
  };
  return <section ref={root} tabIndex={-1} data-search-result={tab.id} aria-label={t('search.beside.title')} className="flex h-full min-h-0 flex-col bg-bg">
    <header className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border p-2">
      <Button size="sm" variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={back}>{t('search.beside.back')}</Button>
      {href !== undefined && <Button size="sm" variant="ghost" icon={<ExternalLink aria-hidden />} onClick={() => router.history.push(href)}>{t('search.beside.here')}</Button>}
    </header>
    <div className="min-h-0 flex-1"><Suspense fallback={<SkeletonRows rows={6} />}>
      {of.kind === 'chat' ? <Chat chatId={of.id} shown={shown} message={of.anchor ?? null} /> : of.kind === 'place-chat' ? <PlaceChat nodeId={of.id} tabId={tab.id} shown={shown} /> : of.kind === 'document' ? <Document documentId={of.id} block={of.anchor ?? null} tabId={tab.id} shown={shown} /> : of.kind === 'thread' ? <Thread threadId={of.id} item={of.anchor ?? null} tabId={tab.id} shown={shown}
        continuationId={of.continuation === '' ? null : of.continuation ?? null} prompt={of.prompt} focusPrompt={focusPrompt} onVersionSelect={(id, anchor) => {
          const next: Record<string, string> = { ...of, continuation: id ?? '' };
          delete next.anchor; delete next.href; delete next.prompt;
          if (anchor !== undefined) { next.prompt = anchor.inputId; if (anchor.itemId !== null) next.anchor = anchor.itemId; }
          setFocusPrompt((held) => anchor?.focus === true ? { groupId: anchor.groupId, request: (held?.request ?? 0) + 1 } : null);
          useWorkbench.getState().change((layout) => setTabTarget(layout, tab.id, next));
        }} /> : null}
    </Suspense></div>
  </section>;
}

registerView({ name: NAME, title: 'search.beside.title', about: 'search.beside.about', icon: Search, screens: 'all', area: 'main', available: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === NAME && tab.of?.id !== undefined), body: ResultView });
