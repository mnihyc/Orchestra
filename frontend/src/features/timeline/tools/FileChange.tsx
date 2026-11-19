/** Paths and differences in the file's own colours with the changed words marked, in one column or two; each opening in the code view; Undo this change.
 *
 * For each file the change touched: its path, what happened to it, `+a −r`, and the difference in
 * the one component that draws a difference. While the agent is still writing a change the
 * difference grows as it is written; a file that is no text is told by its kind alone; a difference
 * that was cut says so.
 */
import { FileMinus, FilePen, FilePlus, FileSymlink } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { FileChange, Item } from '@/generated/api-types';
import { useTimelineContext } from '@/features/timeline/context';
import { useRecordedItem } from '@/features/timeline/historical';
import { FileLink, FilePathAttachment } from '@/features/files/FileLink';
import { DiffView } from '@/ui/DiffView';

const ICONS = { add: FilePlus, modify: FilePen, delete: FileMinus, rename: FileSymlink } as const;

/** The changes of one step, one difference after the other. `writing`: the step is still under way. */
export function FileChanges({ changes, writing = false, showCounts = true, showHeadings = true }: { changes: readonly FileChange[]; writing?: boolean; showCounts?: boolean; showHeadings?: boolean }) {
  const { t } = useTranslation();
  const { layout, setLayout } = useTimelineContext();
  return (
    <div className="flex flex-col gap-3">
      {changes.map((change, index) => {
        const Icon = ICONS[change.change];
        return (
          <section key={`${change.path}.${index}`} aria-label={change.path} className="flex flex-col gap-1">
            {showHeadings && <p className="flex flex-wrap items-center gap-2 text-sm">
              <Icon className="text-text-muted" aria-hidden />
              <span className="font-mono text-text wrap-anywhere">
                {change.old_path !== null && `${change.old_path} → `}
                {change.change === 'delete' ? change.path : <FileLink path={change.path} />}
              </span>
              <span className="text-text-subtle">{t(`timeline.change.${change.change}`)}</span>
              {showCounts && !change.binary && (
                <span className="text-xs text-text-muted tabular-nums">
                  <span className="text-success-text">{`+${change.added}`}</span> <span className="text-danger-text">{`−${change.removed}`}</span>
                </span>
              )}
            </p>}
            {change.binary && change.change !== 'delete' && <FilePathAttachment path={change.path} />}
            <DiffView
              diff={change.diff}
              change={change.change}
              binary={change.binary}
              truncated={change.truncated}
              writing={writing}
              layout={layout}
              onLayout={setLayout}
              label={t('timeline.change.label', { path: change.path })}
              showCounts={showCounts}
              showToolbar={showHeadings}
            />
          </section>
        );
      })}
    </div>
  );
}

type ChangeItem = Extract<Item, { kind: 'tool_call' }>;

export function FileChangeDisplay({ item }: { item: ChangeItem }) {
  const { threadId } = useTimelineContext();
  const recorded = useRecordedItem(threadId, item);
  const tool = item.payload.tool;
  if (tool.kind !== 'file_change') return null;
  return <FileChanges changes={tool.changes} writing={item.status === 'in_progress' && !recorded} showCounts={false} showHeadings={tool.changes.length !== 1} />;
}
