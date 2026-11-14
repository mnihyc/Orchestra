/** A place is chosen in the same hierarchy as the left bar, with local search and explicit folding. */
import { useQueries, useQuery } from '@tanstack/react-query';
import { ChevronDown, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchTree, orgsQuery } from '@/api/nodes';
import type { AdminAccess, NodeKind, TreeNode } from '@/generated/api-types';
import { useTreeStore } from '@/realtime/stores/tree';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { buildRows, newLayoutMemory } from '@/features/navigation/treeRows';
import { Popover } from '@/ui/Popover';
import { Tree, type TreeRow } from '@/ui/Tree';
import { Input, fieldBoxClasses } from '@/ui/Input';
import { Field } from '@/ui/Field';
import { IconButton } from '@/ui/IconButton';
import { Choice, type ChoiceHelp } from '@/ui/Choice';
import { NodeIcon } from '@/ui/NodeIcon';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';

const NO_ROWS: readonly TreeNode[] = [];

/** A place as the picker offers it. */
export interface PickedPlace {
  adminAccess?: AdminAccess;
  /** Actual root of the native tree which supplied this place. */
  rootOrgId?: string;
  rootName?: string;
  id: string;
  kind: NodeKind;
  /** The colour it was given, 1 to 12; `null` for the colour of its kind. */
  color: number | null;
  /** A colour of its own, where the place has one. */
  custom_color?: string | null;
  /** A project that lies in a project. */
  folder: boolean;
  /** "Acme / Research / Web". */
  path: string;
}

/** The places of one tree as paths of names, in the order of the tree. */
export function placesOfTree(rows: readonly TreeNode[], separator: string, includeThreads = false): PickedPlace[] {
  const byId = new Map(rows.map((row) => [row.id, row]));
  const pathOf = (row: TreeNode): string => {
    const names: string[] = [];
    const seen = new Set<string>();
    for (let at: TreeNode | undefined = row; at !== undefined && !seen.has(at.id); at = at.parent_id === null ? undefined : byId.get(at.parent_id)) {
      seen.add(at.id);
      names.unshift(at.name);
    }
    return names.join(separator);
  };
  return rows
    .filter((row) => !row.deleted && (includeThreads || row.kind !== 'thread'))
    .map((row) => {
      const adminAccess = adminAccessOf(row);
      return { id: row.id, kind: row.kind, color: row.color, ...(row.custom_color === undefined ? {} : { custom_color: row.custom_color }), folder: row.kind === 'project' && (row.parent_id === null ? undefined : byId.get(row.parent_id))?.kind === 'project', path: pathOf(row), ...(adminAccess === null ? {} : { adminAccess }) };
    })
    .sort((one, other) => one.path.localeCompare(other.path));
}

export interface PlacePickerProps {
  label: string;
  hint?: string;
  value: PickedPlace | null;
  onChange: (place: PickedPlace | null) => void;
  disabled?: boolean;
  /** Restricts environment homes to their existing root org. */
  rootOrgId?: string;
  /** Offers only places of this kind: the topic a thread is made in. */
  only?: NodeKind;
  /** Settings can be applied to a thread as well as the places above it. */
  includeThreads?: boolean;
  /** Native destination rules keep an unavailable place visible, with its own reason. */
  refusal?: (place: PickedPlace) => string | false;
  onCandidatesChange?: (places: readonly PickedPlace[]) => void;
  optionHelp?: (place: PickedPlace) => ChoiceHelp | undefined;
  optionDescription?: (place: PickedPlace) => string | undefined;
}

export function PlacePicker({ label, hint, value, onChange, disabled = false, rootOrgId, only, includeThreads = false, refusal, onCandidatesChange, optionHelp, optionDescription }: PlacePickerProps) {
  const { t, i18n } = useTranslation();
  const search = useRef<HTMLInputElement>(null);
  const holder = useRef<HTMLDivElement>(null);
  const [closed, setClosed] = useState<ReadonlySet<string>>(() => new Set());
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState(false);
  const orgs = useQuery({ ...orgsQuery, enabled: open && value === null });
  const live = useTreeStore((state) => state.byOrg);
  const roots = useMemo(() => (orgs.data ?? []).filter((org) => rootOrgId === undefined || org.id === rootOrgId), [orgs.data, rootOrgId]);
  const heldAt = (index: number) => { const id = roots[index]?.id; return id === undefined ? undefined : live.get(id); };
  const trees = useQueries({
    queries: roots.map((org) => ({
      queryKey: ['tree-places', org.id] as const,
      queryFn: ({ signal }: { signal: AbortSignal }) => fetchTree(org.id, null, signal),
      enabled: open && value === null && live.get(org.id)?.loaded !== true,
    })),
    // The rows of every tree as one value, which stays the same object while nothing in it changed.
    combine: (results) => ({ trees: results.map((result, index) => {
      const held = heldAt(index);
      return held?.loaded === true ? [...held.nodes.values()] : result.data?.nodes ?? NO_ROWS;
    }), pending: results.some((result, index) => heldAt(index)?.loaded !== true && result.isPending),
    failed: results.find((result, index) => heldAt(index)?.loaded !== true && result.isError),
    retry: () => Promise.all(results.filter((result) => result.isError).map((result) => result.refetch())) }),
  });
  const loading = open && (orgs.isPending || trees.pending);
  const places = useMemo(() => {
    const separator = t('settings.place.separator');
    return trees.trees.flatMap((rows, index) => {
      const root = roots[index];
      const namedRoot = root?.path.find((step) => step.id === root.org_id && !step.redacted);
      const rootName = root?.id === root?.org_id ? root?.name : namedRoot?.name;
      return placesOfTree(rows, separator, includeThreads).map((place) => ({ ...place, ...(root === undefined ? {} : { rootOrgId: root.org_id, ...(rootName === null || rootName === undefined ? {} : { rootName }) }) }));
    });
  }, [trees.trees, roots, t, includeThreads]);
  const names = useMemo(() => new Map(trees.trees.flatMap((nodes) => nodes.map((node) => [node.id, node.name] as const))), [trees.trees]);
  const needle = query.trim().toLocaleLowerCase();
  const candidates = useMemo(() => places.filter((place) => only === undefined || place.kind === only)
    .filter((place) => needle === '' || place.path.toLocaleLowerCase().includes(needle)), [places, only, needle]);
  const candidateIds = useMemo(() => new Set(candidates.map((place) => place.id)), [candidates]);
  const rows = useMemo(() => {
    const byId = new Map(places.map((place) => [place.id, place]));
    const wanted = candidateIds;
    return trees.trees.flatMap((nodes, index): TreeRow<PickedPlace>[] => {
      const root = roots[index];
      if (root === undefined) return [];
      const all = new Map(nodes.filter((node) => !node.deleted && (includeThreads || node.kind !== 'thread')).map((node) => [node.id, node]));
      const included = new Set<string>();
      for (const id of all.keys()) {
        if (!wanted.has(id)) continue;
        let at: string | null = id;
        while (at !== null && !included.has(at)) {
          const node = all.get(at);
          if (node === undefined) break;
          included.add(at); at = node.parent_id;
        }
      }
      const filtered = new Map([...all].filter(([id]) => included.has(id)));
      const expanded = new Set([...filtered.keys()].filter((id) => needle !== '' || !closed.has(id)));
      const laid = buildRows({ nodes: filtered, loaded: true, lastSeq: 0, error: null }, root.org_id, expanded, i18n.language, { showArchived: true, threads: includeThreads }, newLayoutMemory());
      const nativeRoot = filtered.get(root.org_id);
      const below = nativeRoot === undefined ? laid : closed.has(root.org_id) && needle === '' ? [] : laid.map((row) => ({ ...row, depth: row.depth + 1, parentId: row.parentId ?? root.org_id }));
      const first: TreeRow<PickedPlace>[] = [];
      const rootPlace = byId.get(root.org_id);
      if (nativeRoot !== undefined && rootPlace !== undefined) first.push({ id: rootPlace.id, item: rootPlace, depth: 0, parentId: null, expandable: laid.length > 0, expanded: expanded.has(rootPlace.id) });
      const nested = below.flatMap((row) => {
        const place = row.item.type === 'node' ? byId.get(row.item.node.id) : undefined;
        return place === undefined ? [] : [{ ...row, item: place }];
      });
      return [...first, ...nested].map((row) => ({ ...row, label: row.item.path,
        description: (wanted.has(row.id) ? refusal?.(row.item) || optionDescription?.(row.item) : undefined) || t(`settings.place.kinds.${row.item.kind}`),
        disabled: !wanted.has(row.id) || Boolean(refusal?.(row.item)) }));
    });
  }, [places, candidateIds, trees.trees, roots, includeThreads, i18n.language, needle, closed, refusal, optionDescription, t]);
  const visible = useMemo(() => {
    const ids = new Set(rows.map((row) => row.id));
    return candidates.filter((place) => ids.has(place.id));
  }, [rows, candidates]);
  useEffect(() => { onCandidatesChange?.(visible); }, [visible, onCandidatesChange]);
  const choose = (row: TreeRow<PickedPlace>) => {
    if (row.disabled === true || disabled) return;
    onChange(row.item); setOpen(false); setQuery('');
  };
  const renderRow = useCallback((row: TreeRow<PickedPlace>) => {
    const place = row.item;
    const eligible = candidateIds.has(place.id);
    const reason = eligible ? refusal?.(place) : false;
    const name = names.get(place.id) ?? place.path;
    return <span className="flex min-w-0 flex-1 items-center gap-2" title={place.path}>
      <NodeIcon kind={place.folder ? 'folder' : place.kind} color={place.color} own={place.custom_color} />
      <Choice className="min-w-0 flex-1" nameClassName="truncate" name={<span className="flex min-w-0 items-center gap-2"><span className="truncate">{name}</span><AsAdminMark of={place.adminAccess?.place} iconOnly /></span>}
        help={eligible ? optionHelp?.(place) : undefined} presentation="picker" />
      {reason && <span className="ml-auto truncate text-xs text-text-muted" title={reason}>{reason}</span>}
      {eligible && !reason && optionDescription?.(place) && <span className="ml-auto truncate text-xs text-text-muted">{optionDescription(place)}</span>}
    </span>;
  }, [refusal, optionHelp, optionDescription, names, candidateIds]);

  if (value !== null) {
    return (
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium text-text">{label}</span>
        <div className="flex items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
          <NodeIcon kind={value.folder ? 'folder' : value.kind} color={value.color} own={value.custom_color} className="size-4" />
          <span className="min-w-0 flex-1 text-base wrap-anywhere text-text">{value.path}</span>
          <AsAdminMark of={value.adminAccess?.place} />
          <IconButton size="sm" label={t('settings.place.clear', { path: value.path })} icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange(null)} />
        </div>
        {refusal?.(value) && <span className="text-sm text-text-muted">{refusal(value)}</span>}
      </div>
    );
  }
  return <Field label={label} hint={hint}>{(control) => <Popover label={label} open={open} onOpenChange={setOpen} scroll="children"
    className="w-(--radix-popover-trigger-width) min-w-64 gap-2 p-2"
    onOpenAutoFocus={(event) => { event.preventDefault(); search.current?.focus(); }}
    trigger={<button type="button" {...control} role="combobox" aria-haspopup="tree" aria-expanded={open} disabled={disabled} className={`${fieldBoxClasses} flex items-center gap-2 text-left`}>
      <span className="min-w-0 flex-1 truncate text-text-subtle">{t('settings.place.placeholder')}</span><ChevronDown aria-hidden className="size-4 shrink-0 text-text-subtle" />
    </button>}>
    <Input ref={search} label={t('settings.place.placeholder')} hideLabel type="search" value={query} onChange={(event) => setQuery(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === 'ArrowDown') { event.preventDefault(); holder.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]')?.focus(); }
        if (event.key === 'Escape' && query !== '') { event.preventDefault(); event.stopPropagation(); setQuery(''); }
      }} />
    {loading ? <p role="status" className="text-sm text-text-muted">{t('common.loading')}</p> : rows.length === 0 ? <p className="text-sm text-text-muted">{t('settings.place.none')}</p> : <div ref={holder} className="min-h-0 max-h-80 overflow-y-auto overscroll-contain">
      <Tree label={label} rows={rows} nameOf={(place) => place.path} renderRow={renderRow} onPick={choose}
        onExpandedChange={(id, expanded) => setClosed((before) => { const next = new Set(before); if (expanded) next.delete(id); else next.add(id); return next; })} />
    </div>}
    {orgs.isError && <ErrorState compact error={orgs.error} action={<Button size="sm" onClick={() => void orgs.refetch()}>{t('common.tryAgain')}</Button>} />}
    {trees.failed !== undefined && <ErrorState compact error={trees.failed.error} action={<Button size="sm" onClick={() => void trees.retry()}>{t('common.tryAgain')}</Button>} />}
  </Popover>}</Field>;
}
