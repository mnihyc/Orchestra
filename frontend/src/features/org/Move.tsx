/** Move to…: an org that lies in another taken elsewhere in its tree, with who gains and who loses what, what resolves differently and how many addresses change said first.
 *
 * What is moved is a project or a folder, a topic, or an org that lies in another. A project and a folder go
 * to another org or into another project or folder; a topic to another project or folder; an org to another
 * org: every one of them within its own tree. The destination is chosen from the places of the tree that
 * can hold it; as soon as one is, the server is asked what the move would change and the answer is written
 * out before anything is moved: who would gain and who lose what on the pages that follow the defaults,
 * which settings would resolve differently and which of those a lock above decides, how many people and
 * resources get another address, and what stops it, with what is missing and whom to ask. Nothing inside what
 * is moved is moved or renamed.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { ArrowRight } from 'lucide-react';
import { type TFunction } from 'i18next';
import { useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { moveNode, type MovableKind, nodeQuery, previewMove } from '@/api/nodes';
import { depthWords } from '@/features/navigation/depth';
import { useNamedPlace } from '@/features/navigation/namedPlace';
import type { PlaceRef } from '@/features/navigation/places';
import type { NodeMovePlan, PlanBlocker, PreviewPerson, TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { permissionLabel } from '@/lib/permissions';
import { useOrgTree } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState, MissingAccessList } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { NodeIcon } from '@/ui/NodeIcon';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { UserChip } from '@/ui/UserChip';

export interface MoveDialogProps {
  /** The project, folder, topic, or org that lies in another, that is moved. */
  place: PlaceRef;
  /** The place it is moved to, chosen already: where a row of the tree was dropped. The plan is shown, and nothing moves until it is confirmed. */
  to?: string;
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
function PlaceChooser({ label, rows, value, onChange, hint }: { label: string; rows: readonly ChooserRow[]; value: string; onChange: (id: string) => void; hint: string | undefined }) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span id={`${id}-label`} className="text-sm font-medium text-text">
        {label}
      </span>
      <div role="radiogroup" aria-labelledby={`${id}-label`} className="flex max-h-72 min-w-0 flex-col overflow-y-auto rounded-lg border border-border py-1">
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

/** What a blocker says: the words the catalogue has for its code, else what the system said. */
function blockerWords(blocker: PlanBlocker, t: TFunction): string {
  const key = `errors.${blocker.code}`;
  // The key is built from a code the system gave, so it is not one the catalogue's types know.
  return i18n.exists(key) ? (t as (key: string) => string)(key) : blocker.message;
}

/** What a person would gain or lose, in words: the permissions by name, the first few, and on how many pages. */
function Change({ entry, change }: { entry: PreviewPerson; change: 'gains' | 'loses' }) {
  const { t } = useTranslation();
  const shown = entry.permissions.slice(0, 3).map(permissionLabel);
  const rest = entry.permissions.length - shown.length;
  const named = shown.join(t('places.counts.join'));
  const what = rest > 0 ? t('folders.move.plan.andMore', { what: named, count: rest }) : named;
  return (
    <li className="flex flex-wrap items-center gap-x-2 gap-y-1">
      <UserChip user={entry.user} size={24} />
      <span className="text-text-muted">{t(`folders.move.plan.${change}`, { what, count: entry.pages })}</span>
    </li>
  );
}

/** What a move would change, written out. */
function Plan({ plan, place }: { plan: NodeMovePlan; place: PlaceRef }) {
  const { t } = useTranslation();
  const lines: string[] = [];
  const { access, roles, addresses } = plan;
  const joined = (...parts: (string | null)[]) => parts.filter((part): part is string => part !== null).join(t('folders.move.plan.join'));
  // What changes for people, in sentences that read right for one person and for many.
  const accessChange = joined(
    access.gaining.length > 0 ? t('folders.move.plan.peopleGain', { count: access.gaining.length }) : null,
    access.losing.length > 0 ? t('folders.move.plan.peopleLose', { count: access.losing.length }) : null,
  );
  if (accessChange !== '') lines.push(t('folders.move.plan.accessOn', { count: access.pages, what: accessChange }));
  const roleChange = joined(
    roles.gaining > 0 ? t('folders.move.plan.rolesStart', { count: roles.gaining }) : null,
    roles.losing > 0 ? t('folders.move.plan.rolesStop', { count: roles.losing }) : null,
  );
  if (roleChange !== '') lines.push(t('folders.move.plan.rolesFrom', { what: roleChange }));
  if (addresses.members + addresses.resources > 0) {
    lines.push(t('places.move.plan.addresses', { members: addresses.members, resources: addresses.resources }));
  }
  const stops = plan.blockers;
  const people = access.gaining.length + access.losing.length > 0 || access.more > 0;
  return (
    <div className="flex flex-col gap-3 text-base">
      <p className="text-sm text-text-muted">
        {t('folders.move.plan.where', { name: plan.node.name ?? place.name, from: plan.from.name ?? '', to: plan.to.name ?? '' })}
      </p>
      {stops.map((blocker) => (
        <InlineAlert key={blocker.code} tone="danger">
          <p>{blockerWords(blocker, t)}</p>
          <MissingAccessList missing={blocker.missing ?? []} className="mt-1" />
        </InlineAlert>
      ))}
      {stops.length === 0 && (
        <>
          <ul className="flex list-disc flex-col gap-1 pl-6">
            {lines.length === 0 && plan.settings.length === 0 && <li>{t('places.move.plan.nothing')}</li>}
            {lines.map((line) => (
              <li key={line}>{line}</li>
            ))}
            {plan.settings.length > 0 && <li>{t('places.move.plan.settings', { count: plan.settings.length })}</li>}
          </ul>
          {people && (
            <ul className="flex flex-col gap-2 text-sm">
              {access.gaining.map((entry) => (
                <Change key={`gain-${entry.user.id}`} entry={entry} change="gains" />
              ))}
              {access.losing.map((entry) => (
                <Change key={`lose-${entry.user.id}`} entry={entry} change="loses" />
              ))}
              {access.more > 0 && <li className="text-text-muted">{t('places.move.plan.more', { count: access.more })}</li>}
            </ul>
          )}
          {plan.settings.length > 0 && (
            <ul className="flex flex-col gap-1 text-sm text-text-muted">
              {plan.settings.map((setting) => (
                <li key={setting.key}>
                  <span className="font-mono text-text">{setting.key}</span>
                  {': '}
                  {JSON.stringify(setting.from)} <ArrowRight className="inline" aria-hidden /> {JSON.stringify(setting.to)}
                  {setting.locked_by !== null &&
                    ` · ${
                      setting.locked_by === 'system' || !('name' in setting.locked_by) || setting.locked_by.name === null
                        ? t('places.move.plan.locked')
                        : t('folders.move.plan.lockedAt', { place: setting.locked_by.name })
                    }`}
                </li>
              ))}
            </ul>
          )}
          {place.kind === 'topic' && <p className="text-sm text-text-muted">{t('folders.move.plan.topicGoes')}</p>}
          <p className="text-sm text-text-muted">{t('places.move.plan.untouched')}</p>
        </>
      )}
    </div>
  );
}

export function MoveDialog({ place, to, open, onClose }: MoveDialogProps) {
  const { t } = useTranslation();
  const named = useNamedPlace(place);
  const navigate = useNavigate();
  const kind: MovableKind = place.kind === 'topic' ? 'topic' : place.kind === 'project' ? 'project' : 'org';
  const node = useQuery(nodeQuery(place.id));
  const tree = useOrgTree(node.data?.org_id ?? null);
  const [destination, setDestination] = useState(to ?? '');
  const options = useMemo(() => destinationsFor(tree.nodes, place, t('places.pathJoin')), [tree.nodes, place, t]);
  const choices = useMemo(() => chooserRows(tree.nodes, options), [tree.nodes, options]);
  const chosen = options.find((option) => option.id === destination);
  // What is moved is called by the word its place has: a project that lies in a project is a folder.
  const word: 'org' | 'project' | 'folder' | 'topic' = place.kind === 'topic' ? 'topic' : place.kind === 'org' ? 'org' : place.folder ? 'folder' : 'project';

  const plan = useQuery({
    queryKey: ['move-preview', place.id, destination],
    queryFn: () => previewMove(kind, place.id, destination),
    enabled: open && destination !== '',
    // What a move would change is what it changes now: a plan is never kept.
    gcTime: 0,
    retry: false,
  });

  const move = useMutation({
    mutationFn: () => {
      if (plan.data === undefined) throw new Error(t('placeReview.reviewAgain'));
      return moveNode(kind, place.id, destination, plan.data.revision, plan.data);
    },
    onError: (error) => {
      if (error instanceof ApiError && (error.code === 'conflict.stale_revision' || error.code === 'preflight.expired')) void plan.refetch();
    },
    onSuccess: (moved) => {
      onClose();
      toast({ tone: 'success', title: t('places.toast.moved', { name: place.name, where: chosen?.path ?? '' }) });
      // An org's address follows its place, so its screen is asked for again by its id.
      if (moved.kind === 'org') void navigate({ to: '/org/$slug', params: { slug: moved.id } });
    },
  });

  const blocked = plan.isError || plan.isFetching || plan.data === undefined || plan.data.blockers.length > 0;
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
          <Button variant="primary" blocked={blocked} pending={move.isPending} className="max-w-full" onClick={() => move.mutate()}>
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
            onChange={setDestination}
            hint={chosen === undefined ? t(`folders.move.choose.${word}`) : chosen.path}
          />
        )}
        {destination !== '' &&
          (plan.isPending ? (
            <SkeletonGroup className="flex flex-col gap-2">
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-1/2" />
            </SkeletonGroup>
          ) : plan.isError ? (
            <ErrorState compact error={plan.error} onRetry={() => void plan.refetch()} />
          ) : (
            <Plan plan={plan.data} place={place} />
          ))}
        {moveError !== null && (
          <ErrorAlert error={moveError} message={moveError instanceof ApiError ? depthWords(moveError, t) : undefined} />
        )}
      </div>
    </Dialog>
  );
}
