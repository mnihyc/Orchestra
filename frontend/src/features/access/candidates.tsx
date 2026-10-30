/** One visible holder picker: current people, groups, roles and members with their source places. */
import { useQuery } from '@tanstack/react-query';
import { ShieldCheck, Users, X } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AccessRowTarget, holderSearchQuery, isNodeKind, SEARCH_MIN_CHARS } from '@/api/access';
import { nodeQuery } from '@/api/nodes';
import { ApiError } from '@/api/client';
import type { Holder, HolderInput, NodeRef, Role, UserRef } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { Avatar } from '@/ui/Avatar';
import { Combobox, type ComboboxGroup, type ComboboxOption } from '@/ui/Combobox';
import { NodeIcon } from '@/ui/NodeIcon';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { isComposing } from '@/lib/shortcuts';

import { holderName } from './holders';

/** A candidate: whom it names, and whether that is somebody beyond the org. */
export interface Candidate {
  input: HolderInput;
  /** The same holder as a page draws it. */
  holder: Holder;
  /** What to call them where they are listed. */
  name: string;
  outside: boolean;
  /** A person, for the "Tell them" tick. */
  user?: UserRef;
}

export interface Candidates {
  groups: ComboboxGroup<string>[];
  byValue: ReadonlyMap<string, Candidate>;
  loading: boolean;
  /** The same place control beside every holder field. */
  placePicker: ReactNode;
}

/** The org a thing lies in or has as its home, which decides who is beyond it. */
export function useTargetOrg(target: AccessRowTarget, enabled = true): string | null {
  const orgId = 'orgId' in target ? target.orgId : undefined;
  const needed = enabled && orgId === undefined && isNodeKind(target.kind);
  const node = useQuery({ ...nodeQuery(target.id), enabled: needed });
  return orgId ?? node.data?.org_id ?? null;
}

const personIcon = (user: UserRef) => (
  <Avatar text={user.avatar.text} color={user.avatar.color} kind={user.kind === 'service' ? 'service' : 'human'} size={24} />
);

export interface CandidateOptions {
  /** A closed picker does not read candidates or source places. */
  enabled?: boolean;
  /** Source-place browsing is optional; the ordinary search still uses the target's place. */
  browsePlaces?: boolean;
  /** Which kinds of holder the field takes. */
  kinds: readonly ('user' | 'group' | 'role' | 'members' | 'node')[];
  /** The place the page names by default, including the topic beside a thread. */
  placeId?: string | undefined;
  /** Disables an outsider with these words, when the caller may not name one. */
  outsiderOff?: string | undefined;
  /** The name of what the page is of, for a role held in it. */
  thing: string;
  /** Roles the field leaves out. */
  withoutRoles?: readonly Role[];
}

/** A visible place selected from a tree; ancestors that are private never supply names. */
interface HolderPlace {
  ref: NodeRef;
  path: string;
}

/** A refused name is not reconstructed from an old path, a locator or node ancestors. */
const unavailable = (error: unknown): boolean => error instanceof ApiError && [401, 403, 404].includes(error.status);
// Refetch can clear error while it is pending. A failed node remains unverified
// until a successful answer, so retry cannot briefly restore cached names.
const unverifiedNode = (query: { isSuccess: boolean; errorUpdatedAt: number }): boolean => !query.isSuccess && query.errorUpdatedAt > 0;

/** This component's query/focus session ends when its owning source control is hidden. */
function HolderPlacePicker({ homeRoot, selected, thing, onSelect }: {
  homeRoot: string | null;
  selected: HolderPlace | null;
  thing: string;
  onSelect: (place: HolderPlace | null) => void;
}) {
  const { t } = useTranslation();
  const [address, setAddress] = useState('');
  const [open, setOpen] = useState(false);
  const text = address.trim(), exact = useDebounced(text);
  const browsing = text === '';
  const searching = text.length >= SEARCH_MIN_CHARS;
  // A chosen source may belong to another tree. Until that node's own root is
  // known, a blank browse must not fall back to the target's home tree.
  const selectedNode = useQuery({ ...nodeQuery(selected?.ref.id ?? ''),
    enabled: open && browsing && selected !== null, retry: false });
  const rootWaiting = browsing && selected !== null && (selectedNode.isPending || selectedNode.isFetching);
  const root = selected === null ? homeRoot : selectedNode.isSuccess && !selectedNode.isFetching ? selectedNode.data.org_id : null;
  const ready = text === exact && (searching || browsing && root !== null);
  const placesQuery = useQuery({ ...holderSearchQuery(searching ? exact : '', searching ? null : root, searching ? homeRoot : root, false, true),
    enabled: open && ready, retry: false });
  const places = useMemo(() => {
    const found = new Map<string, HolderPlace>();
    // No previous query, failed refresh or unverified selected root may lend rows
    // to this input. The native reply itself supplies each safe display path.
    if (!ready || placesQuery.isError || placesQuery.isFetching) return [];
    for (const item of placesQuery.data?.items ?? []) {
      if (item.holder.kind === 'role' && item.holder.at !== null && !item.holder.at.redacted && item.holder.at.name !== null) {
        found.set(item.holder.at.id, { ref: item.holder.at, path: item.place });
      }
    }
    return [...found.values()].sort((one, other) => one.path.localeCompare(other.path));
  }, [placesQuery.data, placesQuery.isError, placesQuery.isFetching, ready]);
  const rootError = browsing && selected !== null ? selectedNode.error : null;
  const searchError = ready ? placesQuery.error : null;
  const error = rootError ?? searchError;
  const retrying = rootError !== null ? selectedNode.isFetching : placesQuery.isFetching;
  const hiddenSelection = unverifiedNode(selectedNode) || unavailable(searchError);
  const selectedName = selected === null ? undefined : hiddenSelection ? t('access.place.hidden') : selected.path;
  const current = t('access.candidates.thisPlace', { place: thing });
  const busy = placesQuery.isFetching || open && (text !== exact || rootWaiting);
  const resetCurrent = () => { setOpen(false); setAddress(''); onSelect(null); };
  const press = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    if (event.key === 'Escape') setAddress('');
    if (event.key !== 'Enter') return;
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  return <div className="flex min-w-[12rem] flex-col gap-2" onKeyDownCapture={press}
    onBlur={(event) => {
      // Keep a failed query and its recovery action mounted through blur,
      // including ErrorState's own access/verification dialog.
      if (error !== null) return;
      if (event.relatedTarget instanceof Node ? !event.currentTarget.contains(event.relatedTarget) : event.target instanceof HTMLInputElement) setAddress('');
    }}>
    <div className="flex min-w-0 items-end gap-1">
    <Combobox fieldClassName="min-w-0 flex-1" label={t('access.candidates.place')} query={address} onQueryChange={setAddress} open={open} onOpenChange={setOpen}
      displayValue={selectedName ?? current} initialActiveValue={selected?.ref.id ?? 'current'}
      placeholder={t('access.candidates.placeAddress')} empty={error === null ? t('access.candidates.placeNone') : undefined}
      loading={busy}
      groups={[{ id: 'places', label: t('access.candidates.places'), options: [
        ...(browsing ? [{ value: 'current', label: current, text: current, disabled: busy }] : []),
        ...(browsing && selected !== null && !places.some((place) => place.ref.id === selected.ref.id)
          ? [{ value: selected.ref.id, label: selectedName ?? current, text: selectedName ?? current, disabled: true }] : []),
        ...places.map((place) => ({ value: place.ref.id, label: place.path, text: place.path, icon: <NodeIcon kind={place.ref.kind} /> })),
      ] }]} onSelect={(option) => {
        setOpen(false);
        if (option.value === 'current' && browsing) onSelect(null);
        else {
          const place = places.find((candidate) => candidate.ref.id === option.value);
          if (place === undefined) return;
          onSelect(place);
        }
        setAddress('');
      }} />
    {/* As in NamePicker, clearing a committed value or draft is a local action,
        outside the list's loading/selection guard. It never waits for a read. */}
    {(selected !== null || address !== '') && <IconButton size="sm" label={current} icon={<X aria-hidden />} onClick={resetCurrent} />}
    </div>
    {error !== null && <ErrorState compact error={error} retrying={retrying} onRetry={() => {
      if (retrying) return;
      // Refetch the failed key once. Do not also reopen/enable it: focus returns
      // to the input only when the person asks for its cached, recovered rows.
      if (rootError !== null) void selectedNode.refetch({ cancelRefetch: false });
      else void placesQuery.refetch({ cancelRefetch: false });
    }} />}
  </div>;
}

/** Selection belongs to this target's source context; query/focus belongs to its mounted picker. */
function useHolderPlace(orgId: string | null, placeId: string | undefined, enabled: boolean, thing: string) {
  const owner = JSON.stringify([orgId, placeId ?? null]);
  const [choice, setChoice] = useState<{ owner: string; place: HolderPlace | null }>({ owner, place: null });
  // A retargeted permission panel starts at its own Current place. This guarded
  // render update resets local state before children can read the old source.
  if (choice.owner !== owner) setChoice({ owner, place: null });
  const selected = choice.owner === owner ? choice.place : null;
  const home = useQuery({ ...nodeQuery(orgId ?? ''), enabled: enabled && orgId !== null });
  const homeRoot = home.data?.org_id ?? orgId;
  // Observe the picker's selected-node result without issuing a second read. A
  // denied source cannot lend old holder candidates to its enclosing field.
  const selectedNode = useQuery({ ...nodeQuery(selected?.ref.id ?? ''), enabled: false });
  const selectedUnavailable = selected !== null && unverifiedNode(selectedNode);
  const picker = enabled ? <HolderPlacePicker key={owner} homeRoot={homeRoot} selected={selected} thing={thing}
    onSelect={(place) => setChoice({ owner, place })} /> : null;
  return { selected, picker, homeRoot, selectedUnavailable };
}

/** The candidates for what has been typed. */
export function useCandidates(query: string, orgId: string | null, options: CandidateOptions): Candidates {
  const { t } = useTranslation();
  const enabled = options.enabled !== false;
  const { selected, picker, homeRoot, selectedUnavailable } = useHolderPlace(orgId, options.placeId, enabled && options.browsePlaces !== false && (options.kinds.includes('group') || options.kinds.includes('role')), options.thing);
  const settled = useDebounced(query);
  const results = useQuery({ ...holderSearchQuery(settled, selected?.ref.id ?? options.placeId ?? orgId, homeRoot, options.kinds.includes('node')),
    enabled: enabled && !selectedUnavailable && query === settled && (options.kinds.some((kind) => kind !== 'user') || settled.trim().length >= SEARCH_MIN_CHARS) });
  const byValue = new Map<string, Candidate>();
  const found: ComboboxOption<string>[] = [];
  for (const item of enabled && !selectedUnavailable && query === settled && !results.isError ? results.data?.items ?? [] : []) {
    const kind = item.input.kind;
    if (!options.kinds.some((wanted) => wanted === kind)) continue;
    if (item.holder.kind === 'role' && options.withoutRoles?.includes(item.holder.role) === true) continue;
    const value = item.input.kind === 'role' ? `role:${item.input.at ?? ''}:${item.input.role}` : 'id' in item.input ? `${item.input.kind}:${item.input.id}` : item.input.kind;
    const name = item.holder.kind === 'group' ? `${item.holder.group.name}, ${item.place}` : item.holder.kind === 'user' ? `${item.holder.user.display_name} · ${item.place}` : holderName(t, item.holder, item.place);
    const user = item.holder.kind === 'user' ? item.holder.user : undefined;
    byValue.set(value, { input: item.input, holder: item.holder, name, outside: item.outside, ...(user === undefined ? {} : { user }) });
    found.push({ value, label: name, text: name, icon: user === undefined ? kind === 'group' ? <Users aria-hidden /> : <ShieldCheck aria-hidden /> : personIcon(user),
      description: t(`access.candidates.kind.${kind === 'role' && item.holder.kind === 'role' && item.holder.role === 'viewer' ? 'everyone' : kind}`),
      disabled: item.outside && options.outsiderOff !== undefined ? options.outsiderOff : false });
  }
  return { groups: [{ id: 'holders', label: t('access.candidates.results'), options: found }], byValue,
    loading: results.isFetching || query !== settled, placePicker: picker };
}
