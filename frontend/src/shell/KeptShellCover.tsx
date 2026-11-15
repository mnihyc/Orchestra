/** Anonymous frame geometry before the session, then only the confirmed reader's kept tabs and tree.
 * The cover has no controls and leaves when the live frame is admitted.
 */
import { Building2, FileText, Folder, ListChecks, MessageSquare, PanelsTopLeft } from 'lucide-react';
import { useMemo } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';

import { type KeptRow, type KeptTab, readKeptShell, readShellGeometry, useCover } from '@/shell/keptShell';
import { cx } from '@/ui/cx';

const KIND_ICONS = { org: Building2, project: Folder, topic: ListChecks, thread: MessageSquare, document: FileText } as const;

function KindMark({ kind, className }: { kind: string; className?: string }) {
  const Icon = Object.hasOwn(KIND_ICONS, kind) ? KIND_ICONS[kind as keyof typeof KIND_ICONS] : PanelsTopLeft;
  return <Icon aria-hidden className={cx('size-4 shrink-0 text-text-muted', className)} />;
}

/** A name that is not known is a bar of the length a name has. */
function Bar({ className }: { className: string }) {
  return <span aria-hidden className={cx('block h-3 animate-skeleton rounded-sm bg-bg-muted', className)} />;
}

function TabMark({ tab }: { tab: KeptTab }) {
  return (
    <div className={cx('flex h-full max-w-56 items-center gap-2 border-b-2 px-3 text-sm', tab.front ? 'border-accent text-text' : 'border-transparent text-text-muted')}>
      <KindMark kind={tab.kind} />
      {tab.title === '' ? <Bar className="w-20" /> : <span className="truncate">{tab.title}</span>}
    </div>
  );
}

function RowMark({ row }: { row: KeptRow }) {
  return (
    <li className="flex h-7 items-center gap-2 pe-2 text-sm" style={{ paddingInlineStart: `${0.5 + row.depth * 0.75}rem` }}>
      <KindMark kind={row.kind} />
      <span className="truncate">{row.name}</span>
    </li>
  );
}

export function KeptShellCover() {
  const covering = useCover((state) => state.covering);
  const reader = useStore(client.session, (state) => state.status === 'signed_in' ? state.userId : null);
  const kept = useMemo(() => reader === null ? null : readKeptShell(reader), [reader]);
  const geometry = useMemo(() => readShellGeometry(), []);
  const frame = kept ?? geometry;
  if (!covering || frame === null) return null;
  const initial = Array.from(kept?.name.trim() ?? '')[0]?.toUpperCase() ?? '';
  return (
    <div aria-hidden inert data-kept-shell className="fixed inset-0 z-50 flex bg-bg text-text">
      <div className="hidden w-(--rail-width) shrink-0 flex-col items-center gap-2 border-r border-border bg-bg-subtle py-3 md:flex">
        {[0, 1, 2, 3].map((one) => <span key={one} className="block size-10 animate-skeleton rounded-lg bg-bg-muted" />)}
        <span className="mt-auto flex size-10 items-center justify-center rounded-full bg-bg-muted text-sm font-medium">{initial}</span>
      </div>
      {frame.left.open && (
        <div className="hidden shrink-0 flex-col gap-1 border-r border-border bg-bg-subtle p-2 md:flex" style={{ width: frame.left.size }}>
          {kept?.tree === null || kept === null ? <Bar className="m-2 w-24" /> : <ul className="min-h-0 flex-1 overflow-hidden">{kept.tree.rows.map((row, index) => <RowMark key={index} row={row} />)}</ul>}
        </div>
      )}
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="h-(--control-md) shrink-0 border-b border-border" />
        <div className="flex h-(--control-md) shrink-0 items-stretch overflow-hidden border-b border-border">
          {(kept?.tabs ?? []).map((tab) => <TabMark key={tab.id} tab={tab} />)}
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
          {[0, 1, 2, 3, 4].map((one) => <Bar key={one} className={one % 2 === 0 ? 'w-2/3' : 'w-1/2'} />)}
        </div>
        <div className="h-6 shrink-0 border-t border-border bg-bg-subtle" />
      </div>
      {frame.right.open && <div className="hidden shrink-0 border-s border-border bg-bg-subtle lg:block" style={{ width: frame.right.size }} />}
    </div>
  );
}
