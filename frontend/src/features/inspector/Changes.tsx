/** The files this thread's turns changed, newest first, each opening as a difference; or those of every thread in its environment.
 *
 * What the agent did to the files, read from the file changes of the tool calls: each file once, with
 * what was added and removed, when, and by which thread. A row opens its latest difference beneath it; its
 * menu opens the difference or the file as a tab, copies the path, and puts the difference into the
 * message. A switch adds the changes of every other thread that works in the same environment. The list
 * is read again a moment after the conversation moves, since it is made of what the conversation does.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ChevronRight, FileDiff, FileMinus, FilePen, FilePlus, FileSymlink, Link2, MessageSquarePlus, MoreHorizontal, ExternalLink } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { changesQuery, type Door } from '@/api/workspace';
import { addToMessage } from '@/features/files/AddToMessage';
import { openDifference, openFile } from '@/features/files/FileTabs';
import type { WorkspaceAccess } from '@/features/files/door';
import type { EnvironmentChange } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { useDebounced } from '@/lib/debounce';
import { cx } from '@/ui/cx';
import { DiffView, type DiffLayout } from '@/ui/DiffView';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { copyText } from '@/ui/clipboard';

import { PathLabel, ThreadViewFrame } from './parts';

const ICONS = { add: FilePlus, modify: FilePen, delete: FileMinus, rename: FileSymlink } as const;

/** How long after the conversation moves the list is read again: the agent writes in bursts. */
const REFRESH_AFTER_MS = 1_000;

/** Asks for the changes again when the conversation moves, and now and then in any case. */
export function useChangesRefresh(door: Door, seq: number): void {
  const client = useQueryClient();
  const settled = useDebounced(seq, REFRESH_AFTER_MS);
  useEffect(() => {
    void client.invalidateQueries({ queryKey: ['workspace-changes', door.environmentId] });
  }, [settled, client, door.environmentId]);
}

function ChangeRow({ change, access, open, onToggle, layout, onLayout }: { change: EnvironmentChange; access: WorkspaceAccess; open: boolean; onToggle: () => void; layout: DiffLayout; onLayout: (layout: DiffLayout) => void }) {
  const { t } = useTranslation();
  const Icon = ICONS[change.change];
  const own = change.thread_id === access.thread.id;
  const entries = (): MenuEntry[] => [
    {
      id: 'diff',
      label: t('inspector.changes.openDifference'),
      icon: <FileDiff aria-hidden />,
      onSelect: () => openDifference({ kind: 'change', environmentId: access.door.environmentId, threadId: access.thread.id, path: change.path, item: change.item_id }),
    },
    {
      id: 'file',
      label: t('inspector.changes.openFile'),
      icon: <ExternalLink aria-hidden />,
      disabled: change.change === 'delete' ? t('inspector.changes.deleted') : false,
      onSelect: () => openFile({ environmentId: access.door.environmentId, threadId: access.thread.id, path: change.path, permanent: false }),
    },
    { id: 'sep', type: 'separator' },
    { id: 'copy', label: t('files.menu.copyPath'), icon: <Link2 aria-hidden />, onSelect: () => void copyText(change.path) },
    {
      id: 'add',
      label: t('files.addToMessage'),
      icon: <MessageSquarePlus aria-hidden />,
      disabled: change.diff === null ? t('inspector.changes.noDifference') : false,
      onSelect: () => addToMessage(access.thread.id, { kind: 'difference', path: change.path, text: change.diff ?? '' }),
    },
  ];
  return (
    <li className="border-b border-border last:border-b-0" data-testid="change-row">
      <div className="group/row flex items-center gap-1 pr-1">
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          className="focus-inset flex min-h-(--row-height) min-w-0 flex-1 items-center gap-2 px-2 text-left text-base hover:bg-bg-muted"
        >
          <ChevronRight className={cx('shrink-0 text-text-subtle transition-transform', open && 'rotate-90')} aria-hidden />
          <Icon className="shrink-0 text-text-muted" aria-hidden />
          <PathLabel path={change.path} />
          {!change.binary && (
            <span className="shrink-0 text-xs tabular-nums">
              <span className="text-success-text">{`+${change.added}`}</span> <span className="text-danger-text">{`−${change.removed}`}</span>
            </span>
          )}
        </button>
        <Menu
          label={t('inspector.changes.menuFor', { path: change.path })}
          trigger={
            <button
              type="button"
              aria-label={t('inspector.changes.menuFor', { path: change.path })}
              className="focus-inset inline-flex size-(--control-sm) shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-border hover:text-text"
            >
              <MoreHorizontal aria-hidden />
            </button>
          }
          entries={entries()}
        />
      </div>
      <p className="px-3 pb-1 text-xs text-text-muted">
        {own ? t('inspector.changes.byThis') : t('inspector.changes.byOther')}{' '}<RelativeTime at={change.at} />
      </p>
      {open && (
        <div className="px-2 pb-2">
          <DiffView
            diff={change.diff}
            change={change.change}
            binary={change.binary}
            truncated={change.truncated}
            layout={layout}
            onLayout={onLayout}
            label={t('files.difference.label', { name: change.path })}
          />
        </div>
      )}
    </li>
  );
}

function ChangesPanel({ access }: { access: WorkspaceAccess }) {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const [open, setOpen] = useState<string | null>(null);
  const [layout, setLayout] = useState<DiffLayout>('one');
  const query = useQuery(changesQuery(access.door, all));
  useChangesRefresh(access.door, access.thread.seq);
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="changes-panel">
      <div className="border-b border-border px-3 py-2">
        <Switch label={t('inspector.changes.all')} tooltip={t('inspector.changes.allAbout')} checked={all} onCheckedChange={setAll} />
      </div>
      {query.error !== null && query.data === undefined ? (
        <ErrorState compact className="flex-1" error={query.error} onRetry={() => void query.refetch()} />
      ) : query.data === undefined ? (
        <SkeletonRows rows={4} />
      ) : query.data.length === 0 ? (
        <EmptyState compact className="flex-1" icon={<FileDiff aria-hidden />} title={all ? t('inspector.changes.noneAll') : t('inspector.changes.none')} description={t('inspector.changes.noneAbout')} />
      ) : (
        <ul className="min-h-0 flex-1 overflow-auto" aria-label={t('inspector.changes.list')}>
          {query.data.map((change) => {
            const key = `${change.item_id}.${change.path}`;
            return <ChangeRow key={key} change={change} access={access} open={open === key} onToggle={() => setOpen(open === key ? null : key)} layout={layout} onLayout={setLayout} />;
          })}
        </ul>
      )}
    </div>
  );
}

/** The Changes view of a thread. */
export function ChangesView(_props: ViewProps) {
  return <ThreadViewFrame need="files.view" icon={<FileDiff aria-hidden />}>{(access) => <ChangesPanel access={access} />}</ThreadViewFrame>;
}
