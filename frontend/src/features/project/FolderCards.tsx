/** The folders of a project, or of a folder, as compact rows following its topics.
 *
 * A folder is a project that lies in a project: a part of the work with its own topics, people and
 * settings, and folders of its own. Rows follow the topics and carry the complete name, counts,
 * attention and the place menu. The authorized live tree already holds these facts; opening this
 * collection reads no detail for each folder. A description is available on the folder's screen.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { AttentionChips } from '@/features/org/ProjectCards';
import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import { PlaceLink, type PlaceStats, placeOfRow, placeStats } from '@/features/navigation/places';
import type { AttentionFilter, TreeNode } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { reloadOrgTree } from '@/realtime/live';
import { type OrgTree, useOrgTree } from '@/realtime/stores/tree';
import { Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { NodeIcon } from '@/ui/NodeIcon';
import { ContainedThreadStateIcon } from '@/ui/ThreadState';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton } from '@/ui/Skeleton';

/** What narrows the folders a project shows, from the address of its screen. */
export interface FolderFilters {
  /** What was typed in the search: a folder is kept when its name has it. */
  q: string;
  /** The tags chosen. A folder carries none, so any tag chosen leaves no folder. */
  tags: number;
  attention: AttentionFilter | 'needs_attention' | null;
  archived: boolean;
  /** The same ordinary-participation focus as the topic list, never administrative reach. */
  participating?: boolean;
}

/** The folders that lie directly in a project or in a folder, by name as the language sorts it. */
export function foldersOf(rows: ReadonlyMap<string, TreeNode>, projectId: string, archived: boolean, locale: string): TreeNode[] {
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  return [...rows.values()]
    .filter((row) => row.parent_id === projectId && row.kind === 'project' && !row.deleted && (archived || !row.archived))
    .sort((one, other) => collator.compare(one.name, other.name));
}

/** Whether a folder stays among the cards as the filters stand: the filters of the topics reach it as far as they can. */
export function folderStays(row: TreeNode, stats: Pick<PlaceStats, 'running' | 'waiting' | 'blocked'>, filters: FolderFilters): boolean {
  if (filters.participating === true && row.participating !== true) return false;
  if (filters.tags > 0) return false;
  if (filters.q.trim() !== '' && !row.name.toLocaleLowerCase().includes(filters.q.trim().toLocaleLowerCase())) return false;
  if (filters.attention === 'running') return stats.running > 0;
  if (filters.attention === 'waiting_approval') return stats.waiting > 0;
  if (filters.attention === 'blocked') return stats.blocked > 0;
  if (filters.attention === 'needs_attention') return stats.waiting > 0 || stats.blocked > 0;
  return true;
}

/** How many topics and folders lie directly in a place, among the rows of a tree; archived ones do not count. */
export function holdings(rows: ReadonlyMap<string, TreeNode>, id: string): { topics: number; folders: number } {
  const held = { topics: 0, folders: 0 };
  for (const row of rows.values()) {
    if (row.parent_id !== id || row.deleted || row.archived) continue;
    if (row.kind === 'topic') held.topics += 1;
    else if (row.kind === 'project') held.folders += 1;
  }
  return held;
}

const CARD =
  'group relative flex min-w-0 items-start gap-3 px-3 py-2 transition-colors hover:bg-bg-subtle';

function FolderCard({ row, tree }: { row: TreeNode; tree: OrgTree }) {
  const { t } = useTranslation();
  const stats = useMemo(() => placeStats(tree.nodes, row.id), [tree.nodes, row.id]);
  const held = useMemo(() => holdings(tree.nodes, row.id), [tree.nodes, row.id]);
  // Names/counts are already held in the authorized tree. A row menu reads details only when opened.
  return (
    <li className={CARD}>
        <span className="flex h-(--control-sm) w-5 shrink-0 items-center justify-center text-text-muted">
          <NodeIcon kind="folder" color={row.color} own={row.custom_color} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1 pt-1">
          <h3 className="min-w-0 text-base">
            <PlaceLink place={row} className="focus-inset block font-medium leading-5 wrap-anywhere text-text hover:underline">
              {row.name}
            </PlaceLink>
          </h3>
      <AttentionChips stats={stats} />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
        <span>{t('places.cards.topics', { count: held.topics })}</span>
        {held.folders > 0 && <span>{t('places.cards.folders', { count: held.folders })}</span>}
        {stats.lastActivity !== null && (
          <RelativeTime at={stats.lastActivity} />
        )}
      </div>
      </div>
      <span className="flex shrink-0 items-center gap-1"><ContainedThreadStateIcon id={row.id} /><NodeMenuButton place={placeOfRow(row, true)} /></span>
    </li>
  );
}

export interface FolderCardsProps {
  /** The project or folder whose folders these are, with the root org that holds it. */
  projectId: string;
  orgId: string;
  /** How many folders the project says it holds, for the cards drawn while the tree is on its way. */
  expected: number;
  filters: FolderFilters;
}

export function FolderCards({ projectId, orgId, expected, filters }: FolderCardsProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const tree = useOrgTree(orgId);
  const folders = useMemo(() => foldersOf(tree.nodes, projectId, filters.archived, locale), [tree.nodes, projectId, filters.archived, locale]);
  const shown = useMemo(
    () => folders.filter((row) => folderStays(row, placeStats(tree.nodes, row.id), filters)),
    [folders, tree.nodes, filters],
  );

  // Nothing to say about folders where there are none, and the tree has said so.
  if ((tree.loaded || expected === 0) && folders.length === 0) return null;
  const narrowed = folders.length - shown.length;
  return (
    <Section id="project-folders" title={t('folders.title')} lead={narrowed > 0 ? t('folders.narrowed', { count: narrowed }) : undefined}>
      {!tree.loaded && tree.error === null ? (
        <ul className="rounded-lg border border-border bg-surface" aria-hidden>
          {Array.from({ length: Math.min(Math.max(expected, 1), 3) }, (_, index) => (
            <li key={index} className={CARD}>
              <Skeleton className="h-5 w-1/2" />
              <Skeleton className="h-4 w-3/4" />
            </li>
          ))}
        </ul>
      ) : !tree.loaded ? (
        <ErrorState compact error={tree.error} onRetry={() => reloadOrgTree(orgId)} />
      ) : shown.length === 0 ? (
        <EmptyState compact title={t('folders.noneMatch')} />
      ) : (
        <ul data-places-list className="divide-y divide-border rounded-lg border border-border bg-surface">
          {shown.map((row) => (
            <FolderCard key={row.id} row={row} tree={tree} />
          ))}
        </ul>
      )}
    </Section>
  );
}
