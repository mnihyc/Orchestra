/** A patch and every comparison: two sides or one column, changed words marked.
 *
 * A `.diff` or `.patch` file is drawn as the difference it holds. The same drawing serves a change a
 * turn recorded and what Git says is uncommitted: the tab of a difference reads the one it was opened
 * for and draws it in the file's own colours, in one column or two, with the words that changed marked.
 */
import { useQuery } from '@tanstack/react-query';
import { FileDiff, MessageSquarePlus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { changesQuery, gitDiff, partOfDiff, workspaceKeys } from '@/api/workspace';
import { CommentOnLines } from '@/features/source/Review';
import { useTabName } from '@/features/workbench/store';
import type { ViewProps } from '@/features/workbench/views';
import { DiffView, type DiffLayout } from '@/ui/DiffView';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';

import { addToMessage } from '../AddToMessage';
import { useFileText } from '../content';
import { useWorkspaceDoor, workspaceDenial } from '../door';
import { baseName, differenceOf } from '../FileTabs';
import type { ViewerProps } from '../viewers';

/** A difference fills the place it is in, and is scrolled there and not in itself. */
const FILLS = { '--diff-max-height': 'none' } as React.CSSProperties;

/** The viewer of a `.diff` or `.patch` file. */
export default function Difference({ door, path, head }: ViewerProps) {
  const { t } = useTranslation();
  const loaded = useFileText(door, path, head.brought);
  const [layout, setLayout] = useState<DiffLayout>('one');
  if (loaded.error !== null) return <ErrorState compact className="h-full" error={loaded.error} />;
  if (loaded.text === null) return <SkeletonRows rows={8} />;
  return (
    <div className="h-full overflow-auto p-2" style={FILLS}>
      <DiffView diff={loaded.text.text} layout={layout} onLayout={setLayout} label={t('files.difference.label', { name: head.stat.name })} truncated={loaded.text.cut} />
    </div>
  );
}

/** The body of the `difference` view: the difference of one change, or of one path in Git. */
export function DifferenceTab({ tab }: ViewProps) {
  const { t } = useTranslation();
  const of = differenceOf(tab);
  const doorState = useWorkspaceDoor(of?.thread === '' ? null : of?.thread ?? null, of?.env ?? null);
  const [layout, setLayout] = useState<DiffLayout>('one');
  const door = doorState.kind === 'ready' && doorState.access.mayView ? doorState.access.door : null;
  const refusal = doorState.kind === 'ready' && !doorState.access.mayView ? workspaceDenial(t, 'files.view', doorState.access.environment) : null;
  const title = of === null ? '' : baseName(of.path);
  useTabName(tab.id, title === '' ? undefined : t('files.difference.tab', { name: title }), FileDiff);

  const changes = useQuery({ ...changesQuery(door ?? { environmentId: '', threadId: null }, true), enabled: of?.kind === 'change' && door !== null });
  const git = useQuery({
    queryKey: workspaceKeys.gitDiff(door ?? { environmentId: '', threadId: null }, of?.staged ?? false),
    queryFn: ({ signal }) => gitDiff(door ?? { environmentId: '', threadId: null }, of?.staged ?? false, signal),
    // The server hands over the difference of the whole repository: the file's part is cut out of it.
    select: (whole) => ({ text: partOfDiff(whole.text, of?.path ?? ''), truncated: whole.truncated }),
    enabled: of?.kind === 'git' && door !== null,
  });

  if (doorState.kind === 'error') return <ErrorState compact className="h-full" error={doorState.error} />;
  if (refusal !== null) return <EmptyState compact className="h-full" icon={<FileDiff aria-hidden />} title={refusal} />;
  if (of === null || doorState.kind === 'none') return <EmptyState compact className="h-full" icon={<FileDiff aria-hidden />} title={t('files.difference.unknown')} />;
  if (doorState.kind === 'loading' || door === null) return <SkeletonRows rows={6} />;
  if (of.kind === 'git') {
    if (git.error !== null) return <ErrorState compact className="h-full" error={git.error} onRetry={() => void git.refetch()} />;
    if (git.data === undefined) return <SkeletonRows rows={8} />;
    return (
      <div className="flex h-full min-h-0 flex-col">
        <Header path={of.path} threadId={door.threadId} text={git.data.text} />
        <div className="min-h-0 flex-1 overflow-auto p-2" style={FILLS}>
          <DiffView diff={git.data.text === '' ? null : git.data.text} layout={layout} onLayout={setLayout} label={t('files.difference.label', { name: of.path })} truncated={git.data.truncated} />
        </div>
      </div>
    );
  }
  if (changes.error !== null) return <ErrorState compact className="h-full" error={changes.error} onRetry={() => void changes.refetch()} />;
  if (changes.data === undefined) return <SkeletonRows rows={8} />;
  const change = changes.data.find((candidate) => candidate.path === of.path && (of.item === '' || candidate.item_id === of.item));
  if (change === undefined) return <EmptyState compact className="h-full" icon={<FileDiff aria-hidden />} title={t('files.difference.gone')} />;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <Header path={change.path} threadId={door.threadId} text={change.diff ?? ''} item={change.item_id} />
      <div className="min-h-0 flex-1 overflow-auto p-2" style={FILLS}>
        <DiffView
          diff={change.diff}
          change={change.change}
          binary={change.binary}
          truncated={change.truncated}
          layout={layout}
          onLayout={setLayout}
          label={t('files.difference.label', { name: change.path })}
        />
      </div>
    </div>
  );
}

function Header({ path, threadId, text, item = null }: { path: string; threadId: string | null; text: string; item?: string | null }) {
  const { t } = useTranslation();
  return (
    <div className="flex min-h-(--control-md) items-center gap-1 border-b border-border px-2 text-sm">
      <span className="min-w-0 flex-1 truncate font-mono text-text-muted" title={path}>
        {path}
      </span>
      {threadId !== null && <CommentOnLines threadId={threadId} path={path} diff={text} itemId={item} />}
      {threadId !== null && <IconButton
        size="sm"
        label={t('files.addToMessage')}
        icon={<MessageSquarePlus aria-hidden />}
        disabled={text === ''}
        onClick={() => addToMessage(threadId, { kind: 'difference', path, text })}
      />}
    </div>
  );
}
