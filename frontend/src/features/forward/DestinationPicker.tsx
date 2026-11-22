/** Forward destinations keep their native admission, recent choices and independent continuations. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { personPickerQuery } from '@/api/access';
import { orgChatsQuery } from '@/api/chats';
import { pickerNamesQuery } from '@/api/search';
import type { Chat, ChatListScope } from '@/generated/api-types';
import { groupDestinations, personDestinations, nodeDestinations, threadDestinations, type Destination } from '@/features/forward/destinations';
import { useDebounced } from '@/lib/debounce';
import { usePickerCursorRestart } from '@/lib/pickerPages';
import { usePickerRecents } from '@/lib/pickerRecents';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { ResourcePickerLookup, type ResourceLookupOption } from '@/ui/ResourcePicker';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import { UserChip } from '@/ui/UserChip';

function useChatChoices(org: string, scope: ChatListScope, q: string, ids: readonly string[], enabled: boolean) {
  const source = orgChatsQuery(org, { scope, of: null, q, ...(scope === 'nodes' ? { includeEmpty: true } : {}) });
  const page = useInfiniteQuery({ ...source, enabled, staleTime: 0, refetchOnWindowFocus: false });
  usePickerCursorRestart(page, source.queryKey);
  const savedSource = orgChatsQuery(org, { scope, of: null, ids, ...(scope === 'nodes' ? { includeEmpty: true } : {}) });
  const saved = useInfiniteQuery({ ...savedSource, enabled: enabled && q === '' && ids.length > 0, staleTime: 0, refetchOnWindowFocus: false });
  usePickerCursorRestart(saved, savedSource.queryKey);
  // Only the configured short Recent set is completed; the ordinary inventory stays explicitly paged.
  const { hasNextPage: savedMore, isFetching: savedFetching, isError: savedError, fetchNextPage: readSaved } = saved;
  useEffect(() => {
    if (enabled && q === '' && ids.length > 0 && savedMore && !savedFetching && !savedError) void readSaved();
  }, [enabled, q, ids.length, savedMore, savedFetching, savedError, readSaved]);
  const rows = new Map<string, Chat>();
  if (q === '') for (const row of saved.data?.pages.flatMap((page) => page.items) ?? []) rows.set(row.id, row);
  for (const row of page.data?.pages.flatMap((page) => page.items) ?? []) rows.set(row.id, row);
  return { page, saved, rows: [...rows.values()], loading: page.isPending || q === '' && ids.length > 0 && !saved.isError && (saved.isPending || saved.hasNextPage) };
}

export function DestinationPicker({ orgId, me, sourceThread, ownChat, chosen, disabled, onChoose, icon }: {
  orgId: string; me: string | null; sourceThread: string | null; ownChat: Extract<Destination, { kind: 'group' }> | null;
  chosen: readonly Destination[]; disabled: boolean; onChoose: (destination: Destination) => void; icon: (destination: Destination) => ReactNode;
}) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState(''), [open, setOpen] = useState(false);
  const q = useDebounced(typed.trim()), waiting = q !== typed.trim();
  // The common picker opens the one preference read; this observer consumes that same batch.
  const recent = usePickerRecents(false), ids = recent.choices?.chat ?? [];
  const enabled = open && !waiting;
  const groups = useChatChoices(orgId, 'mine', q, ids, enabled);
  const nodes = useChatChoices(orgId, 'nodes', q, ids, enabled);
  const personSource = personPickerQuery(q, orgId);
  const people = useInfiniteQuery({ ...personSource, enabled });
  usePickerCursorRestart(people, personSource.queryKey);
  const threadSource = pickerNamesQuery({ q, kinds: ['thread'], orgId });
  const threads = useInfiniteQuery({ ...threadSource, enabled });
  usePickerCursorRestart(threads, threadSource.queryKey);
  const persons = people.data?.pages.flatMap((page) => page.items) ?? [];
  const destinations: Destination[] = [
    ...groupDestinations(groups.rows, '', me),
    ...personDestinations(persons, me),
    ...threadDestinations(threads.data?.pages.flatMap((page) => page.items) ?? [], sourceThread),
    ...nodeDestinations(nodes.rows),
  ];
  const held = new Set(chosen.map((row) => row.key));
  const byKey = new Map(destinations.filter((row) => !held.has(row.key) && row.key !== ownChat?.key).map((row) => [row.key, row]));
  const options: ResourceLookupOption<string>[] = [...byKey.values()].map((row) => {
    const person = row.kind === 'person' ? persons.find((user) => user.id === row.userId) : undefined;
    return { value: row.key, text: row.label, label: person === undefined ? row.label : <UserChip user={person} size={24} card={false} />, icon: person === undefined ? icon(row) : undefined,
      resource: { kind: row.kind === 'person' ? 'user' : row.kind === 'thread' ? 'thread' : 'chat', id: row.kind === 'person' ? row.userId : row.kind === 'thread' ? row.threadId : row.chatId },
      description: <span className="flex flex-wrap gap-x-2"><span>{t(`forward.kind.${row.kind}`)}</span>{row.detail !== null && <span>{row.detail}</span>}{row.kind === 'thread' && <ThreadIdentity id={row.threadId} />}</span>,
    };
  });
  const special = ownChat === null || held.has(ownChat.key) || typed.trim() !== '' && !ownChat.label.toLocaleLowerCase().includes(typed.trim().toLocaleLowerCase()) ? [] : [{ value: ownChat.key, text: ownChat.label, label: ownChat.label, icon: icon(ownChat) }];
  const sources = [{ kind: 'group', read: groups.page }, { kind: 'person', read: people }, { kind: 'thread', read: threads }, { kind: 'node', read: nodes.page }] as const;
  const recover = [{ kind: 'group', read: groups.saved }, { kind: 'node', read: nodes.saved }] as const;
  const footer = <div className="flex flex-wrap gap-1">
    {sources.map(({ kind, read }) => read.isError ? <ErrorState key={kind} compact title={t(`forward.kind.${kind}`)} error={read.error}
      onRetry={() => { void (read.isFetchNextPageError ? read.fetchNextPage() : read.refetch()); }} /> : read.hasNextPage ? <Button key={kind} size="sm" variant="ghost" pending={read.isFetchingNextPage}
        onClick={() => { void read.fetchNextPage(); }}>{t('forward.more', { kind: t(`forward.kind.${kind}`) })}</Button> : null)}
    {q === '' && ids.length > 0 && recover.map(({ kind, read }) => read.isError ? <ErrorState key={`saved-${kind}`} compact title={t('ui.picker.recent')} error={read.error} onRetry={() => { void read.refetch(); }} /> : null)}
  </div>;
  return <ResourcePickerLookup pickerKey="forward-destinations" groupHomes={false} filterLocal={false} disabled={disabled}
    label={t('forward.search')} placeholder={t('forward.search')} displayValue="" query={typed} onQueryChange={setTyped} onOpenChange={setOpen}
    loading={waiting || enabled && (groups.loading || nodes.loading || people.isPending || threads.isPending)}
    empty={t('forward.nothing')} groups={[{ id: 'forward-home', label: '', options: special }, { id: 'forward-choices', label: t('forward.found'), options: enabled ? options : [] }]}
    footer={sources.some(({ read }) => read.isError || read.hasNextPage) || q === '' && ids.length > 0 && recover.some(({ read }) => read.isError) ? footer : undefined}
    onSelect={(option) => { const row = option.value === ownChat?.key ? ownChat : byKey.get(option.value); if (row !== undefined) { if (option.value === ownChat?.key) recent.record({ kind: 'chat', key: ownChat.chatId }); setOpen(false); onChoose(row); setTyped(''); } }} />;
}
