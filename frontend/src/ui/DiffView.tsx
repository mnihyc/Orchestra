/** Shared DiffView component (content): a difference in the file's own colours with the changed words marked, in one column or two.
 *
 * The one component that draws a difference, in a step of the conversation, in an approval and in
 * the views that list changes. It is drawn the way a code host draws one: line numbers on both sides,
 * added lines tinted and removed lines tinted, and inside a changed line the words that changed marked
 * more strongly; long stretches that did not change are folded to a line that says how many. A
 * difference is shown in one column, or in two side by side where there are 900 px for them.
 */
import { Columns2, Rows2 } from 'lucide-react';
import { memo, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { countChanges, type DiffLine, type Hunk, parseDiff, sideBySide, unchangedBetween } from '@/lib/diff';
import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { ScrollRegion } from '@/ui/ScrollRegion';

/** The width two columns need to be read: each side keeps a line of code readable. */
export const TWO_COLUMNS_NEED_PX = 900;

export type DiffLayout = 'one' | 'two';

export interface DiffViewProps {
  /** The difference as unified text; `null` for a file that is no text and while a change is still being written. */
  diff: string | null;
  /** What was done to the file, which tints a difference that is a whole file. */
  change?: 'add' | 'modify' | 'delete' | 'rename';
  /** The file is not text. */
  binary?: boolean;
  /** The recorded difference was cut. */
  truncated?: boolean;
  /** The change is still being written: the difference grows. */
  writing?: boolean;
  layout?: DiffLayout;
  onLayout?: (layout: DiffLayout) => void;
  /** A name for the difference, for a screen reader. */
  label: string;
  className?: string;
  /** A timeline row already reports the totals once; other consumers keep the diff caption. */
  showCounts?: boolean;
  /** A compact single-file tool keeps its acts on its opening row; other displays retain this toolbar. */
  showToolbar?: boolean;
}

/** A line of text with the ranges that changed marked. */
function Text({ line }: { line: DiffLine }) {
  const marks = line.marks;
  if (marks === undefined || marks.length === 0) return <>{line.text === '' ? ' ' : line.text}</>;
  const parts: React.ReactNode[] = [];
  let at = 0;
  marks.forEach(([from, to], index) => {
    if (from > at) parts.push(line.text.slice(at, from));
    parts.push(
      <mark key={index} className={cx('rounded-sm text-inherit', line.kind === 'add' ? 'bg-success/30' : 'bg-danger/30')}>
        {line.text.slice(from, to)}
      </mark>,
    );
    at = to;
  });
  if (at < line.text.length) parts.push(line.text.slice(at));
  return <>{parts}</>;
}

const TINT = { add: 'bg-success-subtle', del: 'bg-danger-subtle', context: '' } as const;

function Gutter({ n }: { n: number | undefined }) {
  return <td className="w-px min-w-10 px-2 text-right text-xs text-text-muted tabular-nums select-none">{n}</td>;
}

function OneColumn({ hunks, label }: { hunks: readonly Hunk[]; label: string }) {
  const { t } = useTranslation();
  return (
    <table aria-label={label} className="w-full border-collapse font-mono text-sm">
      <tbody>
        {hunks.map((hunk, hunkIndex) => {
          const before = hunks[hunkIndex - 1];
          const skipped = before === undefined ? hunk.oldStart - 1 : unchangedBetween(before, hunk);
          return [
            skipped > 0 && (
              <tr key={`fold-${hunkIndex}`} className="bg-bg-subtle text-text-muted">
                <td colSpan={4} className="px-3 py-0.5 font-sans text-xs">
                  {t('timeline.diff.unchanged', { count: skipped })}
                </td>
              </tr>
            ),
            ...hunk.lines.map((line, index) => (
              <tr key={`${hunkIndex}.${index}`} className={TINT[line.kind]}>
                <Gutter n={line.oldNo} />
                <Gutter n={line.newNo} />
                <td aria-hidden className="w-4 text-center text-text-muted select-none">
                  {line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' '}
                </td>
                <td className={cx('pr-3 whitespace-pre-wrap break-words', line.kind === 'del' && 'line-through decoration-danger/40')}>
                  <span className="sr-only">{t(`timeline.diff.${line.kind}`)} </span>
                  <Text line={line} />
                </td>
              </tr>
            )),
          ];
        })}
      </tbody>
    </table>
  );
}

function TwoColumns({ hunks, label }: { hunks: readonly Hunk[]; label: string }) {
  const { t } = useTranslation();
  return (
    <table aria-label={label} className="w-full table-fixed border-collapse font-mono text-sm">
      <colgroup>
        <col className="w-10" />
        <col />
        <col className="w-10" />
        <col />
      </colgroup>
      <tbody>
        {hunks.map((hunk, hunkIndex) => {
          const before = hunks[hunkIndex - 1];
          const skipped = before === undefined ? hunk.oldStart - 1 : unchangedBetween(before, hunk);
          return [
            skipped > 0 && (
              <tr key={`fold-${hunkIndex}`} className="bg-bg-subtle text-text-muted">
                <td colSpan={4} className="px-3 py-0.5 font-sans text-xs">
                  {t('timeline.diff.unchanged', { count: skipped })}
                </td>
              </tr>
            ),
            ...sideBySide(hunk).map((row, index) => (
              <tr key={`${hunkIndex}.${index}`}>
                <td className={cx('px-2 text-right text-xs text-text-muted tabular-nums select-none', row.left ? TINT[row.left.kind] : 'bg-bg-subtle')}>{row.left?.oldNo}</td>
                <td className={cx('pr-3 pl-1 whitespace-pre-wrap break-words', row.left ? TINT[row.left.kind] : 'bg-bg-subtle')}>
                  {row.left !== null && (
                    <>
                      <span className="sr-only">{t(`timeline.diff.${row.left.kind}`)} </span>
                      <Text line={row.left} />
                    </>
                  )}
                </td>
                <td className={cx('border-l border-border px-2 text-right text-xs text-text-muted tabular-nums select-none', row.right ? TINT[row.right.kind] : 'bg-bg-subtle')}>{row.right?.newNo}</td>
                <td className={cx('pr-3 pl-1 whitespace-pre-wrap break-words', row.right ? TINT[row.right.kind] : 'bg-bg-subtle')}>
                  {row.right !== null && (
                    <>
                      <span className="sr-only">{t(`timeline.diff.${row.right.kind}`)} </span>
                      <Text line={row.right} />
                    </>
                  )}
                </td>
              </tr>
            )),
          ];
        })}
      </tbody>
    </table>
  );
}

function DiffViewBody({ diff, binary = false, truncated = false, writing = false, layout = 'one', onLayout, label, className, showCounts = true, showToolbar = true }: DiffViewProps) {
  const { t } = useTranslation();
  const hunks = useMemo(() => (diff === null ? [] : parseDiff(diff)), [diff]);
  const counts = useMemo(() => countChanges(hunks), [hunks]);
  const box = useRef<HTMLDivElement>(null);
  const [wide, setWide] = useState(false);

  useEffect(() => {
    const element = box.current;
    if (element === null || typeof ResizeObserver === 'undefined') return;
    const watch = new ResizeObserver(() => setWide(element.clientWidth >= TWO_COLUMNS_NEED_PX));
    watch.observe(element);
    setWide(element.clientWidth >= TWO_COLUMNS_NEED_PX);
    return () => watch.disconnect();
  }, []);

  const two = layout === 'two' && wide;
  return (
    <div ref={box} className={cx('min-w-0 overflow-hidden rounded-md border border-border bg-surface', className)}>
      {showToolbar && (diff !== null || (onLayout !== undefined && wide)) && <div className="flex min-h-(--caption-height) items-center justify-between gap-2 border-b border-border bg-bg-subtle px-2 text-xs text-text-muted">
        <span className="tabular-nums">
          {showCounts && diff !== null && (
            <>
              <span className="text-success-text">{`+${counts.added}`}</span> <span className="text-danger-text">{`−${counts.removed}`}</span>
            </>
          )}
        </span>
        <span className="flex items-center gap-0.5">
          {onLayout !== undefined && wide && (
            <IconButton
              size="sm"
              label={two ? t('timeline.diff.oneColumn') : t('timeline.diff.twoColumns')}
              icon={two ? <Rows2 aria-hidden /> : <Columns2 aria-hidden />}
              onClick={() => onLayout(two ? 'one' : 'two')}
            />
          )}
          {diff !== null && <CopyButton value={diff} label={t('timeline.diff.copyPatch')} />}
        </span>
      </div>}
      {diff === null ? (
        <p className="px-3 py-2 text-sm text-text-muted">{binary ? t('timeline.diff.binary') : writing ? t('timeline.diff.writing') : t('timeline.diff.none')}</p>
      ) : hunks.length === 0 ? (
        <EmptyState compact className="px-3" title={t('timeline.diff.empty')} />
      ) : (
        <ScrollRegion kind="code" label={label} className="max-h-(--diff-max-height) overflow-auto">{two ? <TwoColumns hunks={hunks} label={label} /> : <OneColumn hunks={hunks} label={label} />}</ScrollRegion>
      )}
      {truncated && <p className="border-t border-border bg-warning-subtle px-3 py-1 text-xs text-text">{t('timeline.diff.cut')}</p>}
    </div>
  );
}

export const DiffView = memo(DiffViewBody);
