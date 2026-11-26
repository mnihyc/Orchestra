/** Move to…: a project or a folder, a topic, or an org that lies in another, to another place of its tree, at once.
 *
 * What is moved is a project or a folder, a topic, or an org that lies in another. A project and a folder go
 * to another org or into another project or folder; a topic to another project or folder; an org to another
 * org: every one of them within its own tree. Links, shortcuts, threads' places and settings' scopes follow a place by
 * its identity, so a move inside one organization says nothing. Into another organization one line says how many
 * people lose access, with their names on Show: it is read beside the choice and never holds the move up. Whatever
 * the tree itself refuses (a cycle, a depth) is the server's refusal at the move. The result offers Undo.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { moveNode, type MovableKind, nodeKey, nodeQuery, previewMove, readNode } from '@/api/nodes';
import { depthWords } from '@/features/navigation/depth';
import { useNamedPlace } from '@/features/navigation/namedPlace';
import type { PlaceRef } from '@/features/navigation/places';
import type { Node, PagePreview, TreeNode } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { useOrgTree } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { NodeIcon } from '@/ui/NodeIcon';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';
import { UserChip } from '@/ui/UserChip';

export interface MoveDialogProps {
  /** The project, folder, topic, or org that lies in another, that is moved. */
  place: PlaceRef;
  open: boolean;
  onClose: () => void;
}

/** One place of the tree as a place to move to: where it lies by name, and why it is not offered when it is not. */
export interface Destination {
  id: string;
  name: string;
  /** What it is, in the word a text uses for it. */
  kind: 'org' | 'project' | 'folder';
  /** The names of everything above it and its own, joined. */
  path: string;
  /** How many places lie above it in its tree: 0 for a root org. */
  depth: number;
  /** Why it cannot be chosen; `null` for one that can. */
  why: 'here' | 'inside' | null;
}

/** The kind of place each kind of thing lies in, and so the kinds of place it can be moved to. */
const FITS: Record<MovableKind, readonly TreeNode['kind'][]> = {
  org: ['org'],
  project: ['org', 'project'],
  topic: ['project'],
};

/**
 * The places of a tree that can hold `place`, in the order a person reads them, with where each lies and which
 * of them are no place to move it to: the one it lies in already, and itself with everything inside it.
 */
export function destinationsFor(rows: ReadonlyMap<string, TreeNode>, place: PlaceRef, separator: string): Destination[] {
  const kind: MovableKind = place.kind === 'topic' ? 'topic' : place.kind === 'project' ? 'project' : 'org';
  const fits = FITS[kind];
  const children = new Map<string, TreeNode[]>();
  for (const row of rows.values()) {
    if (row.deleted || row.archived || row.parent_id === null || (row.kind !== 'org' && row.kind !== 'project')) continue;
    const list = children.get(row.parent_id);
    if (list === undefined) children.set(row.parent_id, [row]);
    else list.push(row);
  }
  // Orgs inside an org stand before its projects, as they do in the left bar; each by name.
  const rank = (row: TreeNode) => (row.kind === 'org' ? 0 : 1);
  const order = (one: TreeNode, other: TreeNode) => rank(one) - rank(other) || one.name.localeCompare(other.name);
  const roots = [...rows.values()].filter((row) => row.kind === 'org' && row.parent_id === null && !row.deleted).sort(order);
  const found: Destination[] = [];
  const walk = (row: TreeNode, above: string[], inside: boolean, parentKind: TreeNode['kind'] | null) => {
    const path = [...above, row.name];
    const within = inside || row.id === place.id;
    if (fits.includes(row.kind)) {
      found.push({
        id: row.id,
        name: row.name,
        kind: row.kind === 'org' ? 'org' : parentKind === 'project' ? 'folder' : 'project',
        path: path.join(separator),
        depth: above.length,
        why: row.id === place.parentId ? 'here' : within ? 'inside' : null,
      });
    }
    for (const child of (children.get(row.id) ?? []).sort(order)) walk(child, path, within, row.kind);
  };
  for (const root of roots) walk(root, [], false, null);
  return found;
}

/** A row of the chooser: a place that can hold what is moved (greyed, with why, where it may not take it), or a place above such places that cannot hold it, named for the shape of the tree. */
export type ChooserRow =
  | { type: 'place'; destination: Destination }
  | { type: 'above'; id: string; name: string; depth: number; kind: 'org' | 'project' | 'folder' };

/** The destinations as the tree they lie in: each after the places above it that are no choice, each of those named once where it stands. */
export function chooserRows(nodes: ReadonlyMap<string, TreeNode>, options: readonly Destination[]): ChooserRow[] {
  const listed = new Set(options.map((one) => one.id));
  const named = new Set<string>();
  const rows: ChooserRow[] = [];
  for (const option of options) {
    const way: TreeNode[] = [];
    for (let at = nodes.get(option.id)?.parent_id ?? null, steps = 0; at !== null && steps < 64; steps += 1) {
      const above = nodes.get(at);
      if (above === undefined) break;
      way.unshift(above);
      at = above.parent_id;
    }
    way.forEach((above, depth) => {
      if (listed.has(above.id) || named.has(above.id)) return;
      named.add(above.id);
      const kind = above.kind === 'org' ? 'org' : way[depth - 1]?.kind === 'project' ? 'folder' : 'project';
      rows.push({ type: 'above', id: above.id, name: above.name, depth, kind });
    });
    rows.push({ type: 'place', destination: option });
  }
  return rows;
}

/** The indent of a level of the chooser: the step of a level of the left bar. */
const indent = (depth: number): string => `calc(var(--spacing-4) * ${Math.min(depth, 8)} + var(--spacing-2))`;

/**
 * The places of the tree as a tree, each that can hold what is moved a choice and each that cannot take it greyed with
 * why. The arrow keys go from choice to choice; a place above them that holds no such thing is named for the shape of
 * the tree and is no choice.
 */
export function PlaceChooser({ label, rows, value, onChange, hint }: { label: string; rows: readonly ChooserRow[]; value: string; onChange: (id: string) => void; hint: string | undefined }) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span id={`${id}-label`} className="text-sm font-medium text-text">
        {label}
      </span>
      {/* Destination choices retain their own viewport while the move plan grows beneath them. */}
      <div role="radiogroup" aria-labelledby={`${id}-label`} data-scroll-allow="choices" className="flex max-h-72 min-w-0 flex-col overflow-y-auto rounded-lg border border-border py-1">
        {rows.map((row) =>
          row.type === 'above' ? (
            <div key={row.id} className="flex min-h-8 min-w-0 items-center gap-2 pr-2 text-sm text-text-muted" style={{ paddingInlineStart: indent(row.depth) }}>
              <NodeIcon kind={row.kind} />
              <span className="truncate">{row.name}</span>
            </div>
          ) : (
            <div key={row.destination.id} className="flex min-h-8 min-w-0 items-center gap-2 pr-2 hover:bg-bg-muted" style={{ paddingInlineStart: indent(row.destination.depth) }}>
              <input
                id={`${id}-${row.destination.id}`}
                type="radio"
                name={id}
                value={row.destination.id}
                checked={value === row.destination.id}
                disabled={row.destination.why !== null}
                aria-describedby={`${id}-${row.destination.id}-about`}
                className="size-4 shrink-0 accent-(--color-accent-solid)"
                onChange={() => onChange(row.destination.id)}
              />
              <label htmlFor={`${id}-${row.destination.id}`} className={`flex min-w-0 flex-1 items-center gap-2 ${row.destination.why === null ? 'cursor-pointer text-text' : 'cursor-not-allowed text-text-disabled'}`}>
                <NodeIcon kind={row.destination.kind} />
                <span className="truncate">{row.destination.name}</span>
              </label>
              <span id={`${id}-${row.destination.id}-about`} className="shrink-0 text-sm text-text-muted">
                {row.destination.why === null ? t(`folders.kinds.${row.destination.kind}`) : t(`places.move.why.${row.destination.why}`)}
              </span>
            </div>
          ),
        )}
      </div>
      {hint !== undefined && <p className="text-sm wrap-anywhere text-text-muted">{hint}</p>}
    </div>
  );
}

/** What is moved, as the routes name it. */
export function movableKind(place: PlaceRef): MovableKind {
  return place.kind === 'topic' ? 'topic' : place.kind === 'project' ? 'project' : 'org';
}

/** The organization a place belongs to: itself for an org, else the nearest org above it; `null` when the tree does not hold it. */
export function organizationOf(nodes: ReadonlyMap<string, TreeNode>, id: string | null): string | null {
  for (let at = id, steps = 0; at !== null && steps < 64; steps += 1) {
    const row = nodes.get(at);
    if (row === undefined) return null;
    if (row.kind === 'org') return row.id;
    at = row.parent_id;
  }
  return null;
}

/** Whether a move takes the place into another organization: an org always changes the org it lies in, and an unknown side counts as another. */
export function crossesOrganization(nodes: ReadonlyMap<string, TreeNode>, place: PlaceRef, destination: string): boolean {
  if (place.kind === 'org') return true;
  const from = organizationOf(nodes, place.parentId);
  const to = organizationOf(nodes, destination);
  return from === null || to === null || from !== to;
}

/** People who lose access by a move into another organization: one line, the names on Show; nothing while it is read or when it cannot be. */
export function LossLine({ preview }: { preview: PagePreview | undefined }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const losing = preview?.losing ?? [];
  if (losing.length === 0) return null;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <p className="flex flex-wrap items-center gap-2 text-sm text-text-muted">
        <span role="status">{t('places.move.loses', { count: losing.length })}</span>
        <Button size="sm" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>{t(open ? 'actions.hide.label' : 'actions.show.label')}</Button>
      </p>
      {open && <ul className="flex flex-col gap-2 text-sm">{losing.map((entry) => <li key={entry.user.id}><UserChip user={entry.user} size={24} /></li>)}</ul>}
    </div>
  );
}

export interface MoveAsked {
  place: PlaceRef;
  /** The destination's id and the words for where it lies. */
  to: string;
  where: string;
  /** The place as the person saw it, with its revision and where it lay; a drop reads the current one first. */
  before?: Node;
  /** Into another organization; a drop reads who loses access first, the dialog has shown it already as `lost`. */
  across: boolean;
  lost?: number;
}

/** One move, its notice and its Undo, for the dialog and for a row dropped in the tree. */
export function useMovePlace() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cache = useQueryClient();
  const describe = useErrorText();
  const undo = async (place: PlaceRef, from: string, fromName: string) => {
    try {
      const current = await readNode(place.id);
      await moveNode(movableKind(place), place.id, from, current.revision, undefined, newActKey());
      void cache.invalidateQueries({ queryKey: nodeKey(place.id) });
      toast({ tone: 'success', title: t('places.toast.moved', { name: place.name, where: fromName }) });
    } catch (error: unknown) {
      toast({ tone: 'danger', title: t('placeReview.undoChanged'), description: describe(error).message });
    }
  };
  return useMutation({
    mutationFn: async ({ place, to, before, across, lost }: MoveAsked): Promise<{ moved: Node; from: Node; lost: number }> => {
      const from = before ?? await readNode(place.id);
      // The count is a line in the notice; a read that fails leaves it out and the move still runs.
      const losing = lost ?? (across ? await previewMove(movableKind(place), place.id, to).then((plan) => plan.access.losing.length, () => 0) : 0);
      const moved = await moveNode(movableKind(place), place.id, to, from.revision, undefined, newActKey());
      return { moved, from, lost: losing };
    },
    onSuccess: ({ moved, from, lost }, { place, where }) => {
      void cache.invalidateQueries({ queryKey: nodeKey(place.id) });
      const parent = from.parent_id;
      const parentName = from.path.at(-2)?.name ?? '';
      toast({ tone: 'success', title: t('places.toast.moved', { name: place.name, where }),
        ...(lost > 0 ? { description: t('places.move.lost', { count: lost }) } : {}),
        ...(parent === null ? {} : { action: { label: t('places.toast.undo'), run: () => void undo(place, parent, parentName) } }) });
      // An org's address follows its place, so its screen is asked for again by its id.
      if (moved.kind === 'org') void navigate({ to: '/org/$slug', params: { slug: moved.id } });
    },
  });
}

export function MoveDialog({ place, open, onClose }: MoveDialogProps) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const kind = movableKind(place);
  const node = useQuery(nodeQuery(place.id));
  const tree = useOrgTree(node.data?.org_id ?? null);
  const [destination, setDestination] = useState('');
  const options = useMemo(() => destinationsFor(tree.nodes, place, t('places.pathJoin')), [tree.nodes, place, t]);
  const choices = useMemo(() => chooserRows(tree.nodes, options), [tree.nodes, options]);
  const chosen = options.find((option) => option.id === destination && option.why === null);
  // What is moved is called by the word its place has: a project that lies in a project is a folder.
  const word: 'org' | 'project' | 'folder' | 'topic' = place.kind === 'topic' ? 'topic' : place.kind === 'org' ? 'org' : place.folder ? 'folder' : 'project';
  const across = chosen !== undefined && crossesOrganization(tree.nodes, place, chosen.id);

  // Only who loses access is read, and only into another organization; it is a line beside the choice, never a gate.
  const loss = useQuery({
    queryKey: ['move-loss', place.id, destination],
    queryFn: () => previewMove(kind, place.id, destination),
    enabled: open && across,
    gcTime: 0,
    retry: false,
  });

  const move = useMovePlace();
  const send = () => {
    if (chosen === undefined || node.data === undefined) return;
    move.mutate({ place, to: chosen.id, where: chosen.path, before: node.data, across, lost: loss.data?.access.losing.length ?? 0 }, {
      onSuccess: onClose,
      onError: (error) => { if (error instanceof ApiError && error.code === 'conflict.stale_revision') void node.refetch(); },
    });
  };
  const moveError = move.error;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !move.isPending && onClose()}
      title={t('places.move.title', { name: named })}
      description={t(`folders.move.lead.${word}`)}
      dismissible={!move.isPending}
      footer={
        <>
          <Button disabled={move.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" blocked={chosen === undefined || node.data === undefined} pending={move.isPending} className="max-w-full" onClick={send}>
            {/* The way to the place is written under the field; the button names the place alone, so that it fits however deep that is. */}
            <span className="truncate">{chosen === undefined ? t('places.move.submit') : t('places.move.confirm', { name: place.name, where: chosen.name })}</span>
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        {node.isError ? (
          <ErrorState compact error={node.error} onRetry={() => void node.refetch()} />
        ) : (
          <PlaceChooser
            label={t('places.move.to')}
            rows={choices}
            value={destination}
            onChange={(next) => { setDestination(next); move.reset(); }}
            hint={chosen === undefined ? t(`folders.move.choose.${word}`) : chosen.path}
          />
        )}
        {across && <LossLine preview={loss.data?.access} />}
        {moveError !== null && (
          <ErrorAlert error={moveError} message={moveError instanceof ApiError ? depthWords(moveError, t) : undefined} />
        )}
      </div>
    </Dialog>
  );
}
