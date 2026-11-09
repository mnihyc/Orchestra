/** A visible settings consumer and its visible Chat share the independently admitted opening. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useLayoutEffect, useState } from 'react';
import { nodeChatOpeningQuery } from '@/api/chats';
import { settingsKey } from '@/api/settings';
import { nodeKey } from '@/api/nodes';
import type { Node } from '@/generated/api-types';
import { useRequestedChat } from '@/features/chat/focus';
import { settingsNavigation } from '@/features/settings/navigation';
import { contentAddress, parsePageAddress } from '@/features/workbench/contentAddress';
import { useViewInstance } from '@/features/workbench/instance';
import { allTabs, findTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { openingForView, shownViewOpening, useShownViewOpening } from '@/features/workbench/viewOpening';
import { useViewChoice } from '@/lib/viewChoices';

interface ChatReader { shown: boolean; scope: string; chatId: string | null }

export function useDefaultsOpening(nodeId: string | null, chat?: ChatReader, capable = true) {
  const instance = useViewInstance();
  const cache = useQueryClient();
  const state = useWorkbench();
  const owner = instance?.content === true ? instance.id : instance?.headingOwner ?? null;
  const tab = owner === null ? null : findTab(state.layout, owner)?.tab ?? null;
  const href = tab === null ? null : contentAddress(tab);
  const target = href === null ? null : parsePageAddress(href);
  const routeNavigation = target === null ? null : settingsNavigation(target.pathname, target.search);
  const navigation = routeNavigation !== null && (routeNavigation.kind === 'org' || routeNavigation.kind === 'project' || routeNavigation.kind === 'topic') ? routeNavigation : null;
  const unfiltered = target?.search.setting === undefined || target.search.setting === '';
  const known = nodeId === null ? undefined : cache.getQueryData<Node>(nodeKey(nodeId));
  const settingsOnly = navigation?.kind === 'org' && navigation.current === 'members'
    && known?.kind === 'org' && known.access.permissions.includes('members.manage');
  const relevant = capable && unfiltered && nodeId !== null && owner !== null && (navigation?.current === 'defaults' || settingsOnly)
    && (navigation.kind === 'org' || navigation.id === nodeId);
  const candidates = relevant ? allTabs(state.layout).filter((candidate) => {
    if (candidate.view !== 'chat') return false;
    if (candidate.of?.node !== undefined) return candidate.of.node === nodeId;
    if (candidate.scope?.mode === 'fixed') return candidate.scope.target.id === nodeId && candidate.scope.target.kind === navigation.kind;
    return state.activeContent === owner;
  }).map((candidate) => candidate.id) : [];
  const contentOpening = useShownViewOpening(relevant ? [owner] : []);
  const chatOpening = useShownViewOpening(candidates);
  const identity = JSON.stringify([owner, href, nodeId, relevant, settingsOnly]);
  const [settled, setSettled] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (!relevant) return;
    let current = true;
    // Slots settle in this UI commit. No timer or network read supplies a speculative lease.
    queueMicrotask(() => { if (current) setSettled(identity); });
    return () => { current = false; };
  }, [identity, relevant]);
  const previous = cache.getQueryState(['node-chat-opening', nodeId]);
  const settings = nodeId === null ? undefined : cache.getQueryCache().find({ queryKey: settingsKey(nodeId), exact: true });
  // A retained editor can hold a local filter/draft absent from its URL. Only its untouched first
  // opening has the known initial full-editor demand; later openings keep canonical readers.
  const first = (previous?.dataUpdateCount ?? 0) === 0 && (previous?.errorUpdateCount ?? 0) === 0
    && (settings?.state.dataUpdateCount ?? 0) === 0 && (settings?.state.errorUpdateCount ?? 0) === 0 && settings?.isActive() !== true;
  const admitted = relevant && contentOpening !== null && chatOpening !== null;
  const eligible = admitted && first && contentOpening.nonce === openingForView(contentOpening.view);
  const [storedScope] = useViewChoice<string>('chat', 'scope', 'thread');
  const requested = useRequestedChat();
  const linked = target?.search.chat;
  const chatId = chat?.chatId ?? requested ?? (typeof linked === 'string' && linked !== '' ? linked : null);
  const demand = eligible || admitted && settingsOnly ? { defaults: true, ...(settingsOnly ? { settings_only: true } : {}), secret_names: !settingsOnly, default_provider_name: !settingsOnly,
    embeddings_provider_name: !settingsOnly && navigation.kind === 'org' } : undefined;
  const owned = admitted ? { content: contentOpening, chat: chatOpening, href } : null;
  const current = () => {
    if (owned === null) return false;
    if (shownViewOpening(owned.content.view) !== owned.content.nonce || shownViewOpening(owned.chat.view) !== owned.chat.nonce) return false;
    const now = findTab(useWorkbench.getState().layout, owned.content.view);
    return now !== null && contentAddress(now.tab) === owned.href;
  };
  const opening = useQuery({ ...nodeChatOpeningQuery(nodeId ?? '', chat?.scope ?? storedScope, chatId, demand, current),
    enabled: nodeId !== null && (chat === undefined ? eligible : chat.shown) });
  return { opening, waiting: relevant && first && !admitted && candidates.length > 0 && settled !== identity || eligible && opening.isPending && opening.isFetching };
}
