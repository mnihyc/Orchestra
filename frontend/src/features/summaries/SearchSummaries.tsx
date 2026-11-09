/** Search reads existing caches only; each frozen selection belongs to one actual native root. */
import { useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useQueries, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { NodeRef, SearchHit } from '@/generated/api-types';
import { searchSummaryProjectionQuery, summaryReaderKey, type ResourceSummaryTarget } from '@/api/summaries';
import { openAskWithDraft } from '@/features/ask/open';
import { pathOf } from '@/features/finder/destinations';
import { destinationOfHit, keyOfHit } from '@/features/search/hits';
import { Button } from '@/ui/Button';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { SummaryText } from './SummaryText';

export function summaryTargetForHit(hit: SearchHit): ResourceSummaryTarget | null {
  if (hit.kind === 'document') return { kind: 'document', id: hit.document.id };
  if (hit.kind === 'chat_message') return { kind: 'chat', id: hit.chat.id };
  if (hit.kind === 'item') return hit.projection?.follows_active === false ? null : { kind: 'thread', id: hit.thread.id };
  if (hit.node.kind === 'thread' || hit.node.kind === 'topic') return { kind: hit.node.kind, id: hit.node.id };
  return null;
}
export function CachedHitSummary({ hit }: { hit: SearchHit }) {
  const target = summaryTargetForHit(hit);
  return target === null ? null : <CachedSummary target={target} />;
}
function CachedSummary({ target }: { target: ResourceSummaryTarget }) {
  useStore(client.session, summaryReaderKey);
  const query = useQuery(searchSummaryProjectionQuery(target));
  return query.isError || query.data?.text == null ? null : <div className="mt-2 text-sm text-text-muted" data-search-summary><SummaryText text={query.data.text} /></div>;
}
interface Entry { root: string; title: string; href: string; text: string; target: ResourceSummaryTarget | null; paragraph: string | null; inspecting: boolean }
export function frozenSearchAsk(label: string, entries: readonly Pick<Entry, 'title' | 'href' | 'text'>[]): string {
  return [label, ...entries.map((entry) => `${entry.title}\n${entry.href}\n${entry.text.split('\n').map((line) => `> ${line}`).join('\n')}`)].join('\n\n');
}
interface ResultsProps { hits: readonly SearchHit[]; roots: ReadonlyMap<string, NodeRef | null>; slugOfOrg: (id: string) => string | null }
export function AskAboutResults(props: ResultsProps) {
  const reader = useStore(client.session, summaryReaderKey);
  return <ResultsAsk key={reader} {...props} reader={reader} />;
}
function ResultsAsk({ hits, roots, slugOfOrg, reader }: ResultsProps & { reader: string }) {
  const { t } = useTranslation();
  const [frozen, setFrozen] = useState<{ reader: string; entries: Entry[]; unresolved: boolean } | null>(null);
  const targets = new Map<string, ResourceSummaryTarget>();
  for (const hit of hits) { const target = summaryTargetForHit(hit); if (target !== null) targets.set(`${target.kind}:${target.id}`, target); }
  for (const entry of frozen?.entries ?? []) if (entry.target !== null) targets.set(`${entry.target.kind}:${entry.target.id}`, entry.target);
  const selected = [...targets.values()];
  const reads = useQueries({ queries: selected.map((target) => searchSummaryProjectionQuery(target)) });
  const cached = new Map<string, (typeof reads)[number] | undefined>(selected.map((target, index) => [`${target.kind}:${target.id}`, reads[index]] as const));
  const entries: Entry[] = [];
  let unresolved = false;
  for (const hit of hits) {
    const target = summaryTargetForHit(hit);
    const read = target === null ? undefined : cached.get(`${target.kind}:${target.id}`);
    const native = read?.isError === true ? undefined : read?.data;
    const root = native?.root_org_id ?? roots.get(keyOfHit(hit))?.id;
    const destination = destinationOfHit(hit, slugOfOrg);
    if (root === undefined || destination === null) { unresolved = true; continue; }
    const title = hit.kind === 'document' ? hit.document.title : hit.kind === 'chat_message' ? hit.chat.name ?? t('search.hit.chat') : hit.kind === 'item' ? hit.thread.name ?? t('screens.thread.title') : hit.node.name ?? t('ask.organization');
    entries.push({ root, title, href: pathOf(destination), text: native?.text ?? hit.snippet,
      target, paragraph: native?.text ?? null, inspecting: native?.inspecting === true || hit.admin_access != null });
  }
  const sourceState = (entry: Entry): 'inspection' | 'unavailable' | 'loading' | undefined => {
    if (entry.inspecting) return 'inspection';
    if (entry.target === null) return undefined;
    const read = cached.get(`${entry.target.kind}:${entry.target.id}`);
    if (read?.isError === true) return 'unavailable';
    const current = read?.data;
    if (current === undefined) return 'loading';
    if (current.inspecting) return 'inspection';
    if (current.root_org_id !== entry.root || entry.paragraph !== null && current.text !== entry.paragraph) return 'unavailable';
    return undefined;
  };
  const selection = frozen?.entries ?? entries;
  const unknownRoot = frozen?.unresolved ?? unresolved;
  const reasonFor = (group: readonly Entry[]) => {
    if (unknownRoot) return t('summaryBox.resultsRootPending');
    const state = group.map(sourceState).find((state) => state !== undefined);
    return state === 'inspection' ? t('summaryBox.inspection') : state === 'unavailable' ? t('summaryBox.resultsUnavailable') : state === 'loading' ? t('common.loading') : undefined;
  };
  const groups = new Map<string, Entry[]>();
  for (const entry of selection) groups.set(entry.root, [...groups.get(entry.root) ?? [], entry]);
  const ask = (root: string) => {
    if ((frozen?.reader ?? reader) !== summaryReaderKey()) return;
    const held = groups.get(root) ?? [];
    if (reasonFor(held) !== undefined) return;
    if (held.length > 0) openAskWithDraft({ root, text: frozenSearchAsk(t('summaryBox.askResults'), held) });
  };
  const retry = () => {
    if (reader !== summaryReaderKey()) return;
    for (const target of new Set(selection.flatMap((entry) => entry.target === null || sourceState(entry) !== 'unavailable' ? [] : [`${entry.target.kind}:${entry.target.id}`]))) {
      const read = cached.get(target);
      if (read !== undefined) void read.refetch();
    }
  };
  const mayRetry = selection.some((entry) => sourceState(entry) === 'unavailable');
  const label = t('summaryBox.askResults');
  if (groups.size < 2) {
    const root = groups.keys().next().value;
    const blocked = reasonFor(selection);
    return <div className="flex min-w-0 flex-wrap items-center gap-1"><Button size="sm" variant="ghost" blocked={blocked !== undefined || root === undefined} reason={blocked ?? (root === undefined ? t('summaryBox.resultsRootPending') : undefined)} onClick={() => { if (root !== undefined) ask(root); }}>{label}</Button>
      {mayRetry && <Button size="sm" variant="ghost" onClick={retry}>{t('common.tryAgain')}</Button>}
    </div>;
  }
  const rootEntries = [...groups].map(([root, group]): MenuEntry => {
    const name = [...roots.values()].find((value) => value?.id === root && !value.redacted)?.name ?? t('ask.organization');
    return { id: root, label: t('summaryBox.askIn', { name }), description: group[0]?.title, disabled: reasonFor(group) ?? false, onSelect: () => ask(root) };
  });
  return <Menu label={label} onOpenChange={(open) => setFrozen(open ? { reader, entries: entries.map((entry) => ({ ...entry })), unresolved } : null)} trigger={<Button size="sm" variant="ghost">{label}</Button>}
    entries={mayRetry ? [...rootEntries, { id: 'retry-current-sources', label: t('common.tryAgain'), onSelect: retry }] : rootEntries} />;
}
