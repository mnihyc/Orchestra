/** Command, exit state, output as it prints and working directory; one left running shown as such, with Stop; what it was for, where the harness says.
 *
 * The command whole in a code block (unless the line above already says all of it), the directory it
 * ran in, and what it printed in mono type with its colours, following the end while it runs. The output is 24 lines tall with its own scroll and
 * can be expanded. Retained output reads in pages; a lost original interval remains explicit.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronsDownUp, ChevronsUpDown } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchOutput } from '@/api/threads';
import { threadQueryRead } from '@/api/threadRead';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { traceRequest } from '@/api/trace';
import type { Item } from '@/generated/api-types';
import { useTimelineContext } from '@/features/timeline/context';
import { outputRetention } from '@/features/timeline/output';
import { useRecordedItem } from '@/features/timeline/historical';
import { describeStep } from '@/features/timeline/titles';
import { stripAnsi } from '@/lib/ansi';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { AnsiText } from '@/ui/AnsiText';
import { Button } from '@/ui/Button';
import { CodeBlock } from '@/ui/CodeBlock';
import { CopyButton } from '@/ui/CopyButton';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';

type CommandItem = Extract<Item, { kind: 'tool_call' }>;

/** How much of the whole output one "Show all" reads: a page of it. */
export const OUTPUT_PAGE_BYTES = 256 * 1024;

export function CommandDisplay({ item }: { item: CommandItem }) {
  const { t } = useTranslation();
  const { threadId, inspection } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  useThreadReadScope(inspection === undefined ? read : null);
  const recorded = useRecordedItem(threadId, item);
  const locale = useAppearance((state) => state.locale);
  const tool = item.payload.tool;
  const [tall, setTall] = useState(false);
  const [readingOutput, setReadingOutput] = useState(false);
  const scroller = useRef<HTMLPreElement>(null);
  const following = useRef(true);
  const running = item.status === 'in_progress' && !recorded;

  const output = tool.kind === 'command' ? tool.output : null;
  const total = output?.total_len ?? 0;
  const retention = output === null ? null : outputRetention(output);
  const held = retention?.inlineBytes ?? 0;
  const available = retention?.retainedBytes ?? null;
  const more = output?.blob !== undefined && output.blob !== null && (available === null || available > held);

  // Every immutable blob page is read once, under the current thread reader's authority.
  const reading = threadQueryRead(read, ['thread-output', threadId, item.id, output?.blob ?? null]);
  const whole = useInfiniteQuery({
    queryKey: inspection === undefined ? reading.key : ['inspection-output', read.key, threadId, item.id, output?.blob ?? null, inspection.continuationId ?? null],
    initialPageParam: 0,
    queryFn: async ({ pageParam, signal }) => {
      const captured = inspection === undefined ? await reading.capture(signal) : undefined;
      const text = inspection === undefined ? await fetchOutput(read, item.id, pageParam, OUTPUT_PAGE_BYTES, signal, captured, output?.blob ?? undefined)
        : await (await traceRequest(`/threads/${inspection.threadId}/items/${item.id}/output?inspect_deleted=true&offset=${pageParam}&limit=${OUTPUT_PAGE_BYTES}${output?.blob === null || output?.blob === undefined ? '' : `&expected_blob=${encodeURIComponent(output.blob)}`}${inspection.continuationId === undefined ? '' : `&continuation_id=${encodeURIComponent(inspection.continuationId)}`}`, { signal })).text();
      return { offset: pageParam, text, bytes: new TextEncoder().encode(text).length };
    },
    getNextPageParam: (last) => last.bytes === 0 || available !== null && last.offset + last.bytes >= available ? undefined : last.offset + last.bytes,
    enabled: readingOutput && output?.blob !== undefined && output.blob !== null && (inspection !== undefined || reading.enabled),
    staleTime: Infinity,
  });
  const shown = readingOutput && whole.data !== undefined ? whole.data.pages.map((page) => page.text).join('') : (output?.inline ?? '');
  const readBytes = whole.data?.pages.reduce((bytes, page) => bytes + page.bytes, 0) ?? 0;

  useEffect(() => {
    const element = scroller.current;
    if (element !== null && following.current) element.scrollTop = element.scrollHeight;
  }, [shown]);

  if (tool.kind !== 'command' || output === null) return null;
  // The row above is the command as it was typed, when that fits it: saying it again would be noise.
  const title = describeStep(item, t);
  const saidAbove = title.command && title.text === tool.command;
  return (
    <div className="flex flex-col gap-2">
      {!saidAbove && <CodeBlock code={tool.command} lang="sh" />}
      {tool.cwd !== '' && <p className="text-sm text-text-muted">{t('timeline.command.in', { directory: tool.cwd })}</p>}
      {(shown !== '' || running || total > 0 || output.blob !== null) && (
        <div className="overflow-hidden rounded-lg border border-border bg-bg-inset">
          <div className="flex min-h-(--caption-height) items-center justify-between gap-2 border-b border-border px-3 text-xs text-text-muted">
            <span>{t('timeline.command.output')}</span>
            <span className="flex items-center gap-1">
              <IconButton
                size="sm"
                label={tall ? t('timeline.command.shrink') : t('timeline.command.expand')}
                icon={tall ? <ChevronsDownUp aria-hidden /> : <ChevronsUpDown aria-hidden />}
                onClick={() => setTall((on) => !on)}
              />
              <CopyButton value={stripAnsi(shown)} label={t('timeline.command.copyOutput')} />
            </span>
          </div>
          <pre
            ref={scroller}
            tabIndex={0}
            aria-label={t('timeline.command.output')}
            onScroll={(event) => {
              const element = event.currentTarget;
              following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
            }}
            className={cx('overflow-auto p-3 text-sm break-words whitespace-pre-wrap', tall ? 'max-h-[70vh]' : 'max-h-(--output-height)')}
          >
            <AnsiText text={shown} />
          </pre>
          {retention?.missing.map(({ from, to }) => <p key={`${from}/${to}`} className="border-t border-border px-3 py-1 text-xs text-text-muted">{t('timeline.command.outputMissing', { from: String(from), to: String(to) })}</p>)}
          {retention?.unknown === true && <p className="border-t border-border px-3 py-1 text-xs text-text-muted">{t('timeline.command.retainedUnknown')}</p>}
          {more && (!readingOutput || whole.hasNextPage) && (
            <div className="flex items-center gap-2 border-t border-border px-3 py-1 text-xs text-text-muted">
              <span>{!readingOutput ? t('timeline.command.tailOnly', { size: formatBytes(held, locale) }) : t('timeline.command.partial', { size: formatBytes(readBytes, locale), total: formatBytes(available ?? total, locale) })}</span>
              <Button size="sm" variant="ghost" pending={whole.isFetching} onClick={() => { if (readingOutput) void whole.fetchNextPage(); else setReadingOutput(true); }}>
                {!readingOutput ? retention?.full === true ? t('timeline.command.showAll', { size: formatBytes(total, locale) }) : t('timeline.command.showRetained') : t('timeline.command.showMore')}
              </Button>
            </div>
          )}
          {whole.error !== null && <ErrorAlert error={whole.error} className="m-2" />}
        </div>
      )}
    </div>
  );
}
