/** Palette creation uses the ordinary node dialogs. With no suitable context, ask for a place first. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { create, useStore } from 'zustand';
import { client } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { namesQuery, SEARCH_MIN_CHARS } from '@/api/search';
import { describe, KIND_ICONS, wayOf } from '@/features/finder/destinations';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { placeOfNode } from '@/features/navigation/places';
import { labelsOfNames, labelOfName } from '@/features/search/labels';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import type { Node, NodeKind } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { denial, holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Combobox, type ComboboxGroup } from '@/ui/Combobox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';

export type CreationKind = 'thread' | 'document';
interface Request { kind: CreationKind; preferred: string | null; number: number; owner: string | null }
const useCreation = create<{ request: Request | null; next: number }>(() => ({ request: null, next: 0 }));
export function openCreation(kind: CreationKind, preferred: string | null = null): void {
  useCreation.setState((before) => ({ request: { kind, preferred, number: before.next, owner: client.session.getState().userId }, next: before.next + 1 }));
}
export const closeCreation = () => useCreation.setState({ request: null });
export function creationPermission(kind: CreationKind, node: Node): string | null {
  return kind === 'document' ? 'doc.create' : node.kind === 'topic' || node.kind === 'project' ? `${node.kind}.interact` : null;
}

export function CreationHost() {
  const request = useCreation((state) => state.request);
  const owner = useStore(client.session, (state) => state.userId);
  useEffect(() => { if (request !== null && request.owner !== owner) closeCreation(); }, [request, owner]);
  return request?.owner === owner ? <Session key={request.number} request={request} /> : null;
}
function Session({ request }: { request: Request }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const known = useKnownPlaces({ trees: false });
  const [query, setQuery] = useState('');
  const [why, setWhy] = useState<string | null>(null);
  const rested = useDebounced(query.trim(), 150); // Same bounded name lookup as Ctrl+K, never every tree.
  const kinds: readonly NodeKind[] = request.kind === 'thread' ? ['project', 'topic'] : ['org', 'project', 'topic', 'thread'];
  const typed = Array.from(rested).length >= SEARCH_MIN_CHARS;
  // Keep the bounded page's partial signal; a short query must not reuse its former choices.
  const names = useQuery({ ...namesQuery({ q: rested, kinds }), select: (page) => page, enabled: typed });
  const hits = typed ? names.data?.items ?? [] : [];
  const labels = labelsOfNames(hits, known);
  const choose = useMutation({
    mutationFn: async (id: string) => {
      const node = await cache.query(nodeQuery(id));
      if (!active.current || useCreation.getState().request?.number !== request.number || client.session.getState().userId !== request.owner) return;
      const permission = creationPermission(request.kind, node);
      if (permission === null) { setWhy(t('commands.create.threadPlace')); return; }
      if (node.archived || !holds(node.access, permission)) {
        setWhy(node.archived ? t('places.project.archived') : denial(t, permission, node.name, node.owner));
        return;
      }
      closeCreation();
      // The chooser releases its focus trap before the ordinary creation form takes focus.
      setTimeout(() => {
        const current = useCreation.getState();
        if (current.request === null && current.next === request.number + 1 && client.session.getState().userId === request.owner) openNodeDialog({ type: request.kind === 'thread' ? 'new-thread' : 'new-document', parent: placeOfNode(node) });
      }, 0);
    },
  });
  const tried = useRef(false);
  // A preferred place is read only after the act, never as a hidden palette dependency.
  const initial = request.preferred;
  const start = choose.mutate;
  useEffect(() => {
    if (initial !== null && !tried.current) { tried.current = true; start(initial); }
  }, [initial, start]);
  const groups: ComboboxGroup<string>[] = [{ id: 'places', label: t('commands.create.places'), options: hits.map((hit) => {
    const Icon = KIND_ICONS[describe(hit)];
    return { value: hit.id, label: labelOfName(hit, labels), text: labelOfName(hit, labels), description: wayOf(hit, t, labels), icon: <Icon aria-hidden /> };
  }) }];
  return <Dialog open onOpenChange={(open) => { if (!open) closeCreation(); }} size="md" title={t(request.kind === 'thread' ? 'commands.create.thread' : 'commands.create.document')}
    footer={<Button onClick={closeCreation}>{t('common.cancel')}</Button>}>
    <div className="flex flex-col gap-3">
      <Combobox label={t('commands.create.where')} query={query} onQueryChange={setQuery} groups={groups} loading={names.isFetching || choose.isPending}
        placeholder={t('commands.create.find')} empty={names.error === null ? t(rested === '' ? 'commands.create.type' : !typed ? 'finder.namesMinimum' : names.data?.partial === true ? 'finder.namesPartial' : 'commands.create.none') : undefined}
        disabled={choose.isPending} onSelect={(option) => { setWhy(null); choose.mutate(option.value); }} />
      {typed && hits.length > 0 && names.data?.partial === true && <StateLine role="status" compact message={t('finder.namesPartial')} />}
      {why !== null && <StateLine role="status" compact message={why} />}
      {choose.error !== null && <ErrorAlert error={choose.error} />}
      {names.error !== null && <ErrorAlert error={names.error} action={<Button size="sm" onClick={() => void names.refetch()}>{t('common.tryAgain')}</Button>} />}
    </div>
  </Dialog>;
}
