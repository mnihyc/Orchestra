/** A call exactly as its harness made it: its own name for the tool, its arguments and what came back, whole, searched within and copied.
 *
 * The third depth of a step: the call exactly as its harness made it. Its own name for the tool, the
 * arguments it passed (as a tree where they are JSON, with long strings folded and a search that
 * narrows the tree) and what came back, whole, each copyable on its own.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { threadQueryRead } from '@/api/threadRead';
import { fetchRaw } from '@/api/threads';
import { traceRequest } from '@/api/trace';
import { useTimelineContext } from '@/features/timeline/context';
import type { Item, RawCall as RawBody } from '@/generated/api-types';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { CodeBlock } from '@/ui/CodeBlock';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { InlineAlert } from '@/ui/InlineAlert';
import { JsonTree } from '@/ui/JsonTree';
import { SkeletonRows } from '@/ui/Skeleton';
import { FileAttachment } from '@/features/files/FileChip';
import { blobSource } from '@/features/files/source';

type RawItem = Extract<Item, { kind: 'tool_call' }>;

function parse(text: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: JSON.parse(text) as unknown };
  } catch {
    return { ok: false };
  }
}

/**
 * The call exactly as its harness made it. A thread's items do not carry it (it repeats what the item says, and is as
 * long): it is read when it is asked for, and read again when the item has changed since.
 */
export function useRawCall(item: RawItem, enabled = true) {
  const { threadId, inspection } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const reading = threadQueryRead(read, ['thread-raw', threadId, item.id, item.seq_updated]);
  return useQuery({
    queryKey: inspection === undefined ? reading.key : ['inspection-raw', read.key, threadId, item.id, item.seq_updated, inspection.continuationId ?? null],
    queryFn: async ({ signal }): Promise<RawBody> => {
      if (inspection !== undefined) {
        const answer = await traceRequest(`/threads/${inspection.threadId}/items/${item.id}/raw?inspect_deleted=true${inspection.continuationId === undefined ? '' : `&continuation_id=${encodeURIComponent(inspection.continuationId)}`}`, { signal });
        return await answer.json() as RawBody;
      }
      return fetchRaw(read, item.id, signal, await reading.capture(signal));
    },
    enabled: enabled && (inspection !== undefined || reading.enabled),
    staleTime: Infinity,
  });
}

/** What it is read as: the call, once it is read; a line where it could not be, with Try again. */
export function RawCall({ item }: { item: RawItem }) {
  const { t } = useTranslation();
  const raw = useRawCall(item);
  if (raw.data !== undefined) return <RawCallView item={item} raw={raw.data} />;
  return (
    <div data-raw={item.id}>
      {raw.isError ? <ErrorState compact title={t('timeline.raw.readFailure')} error={raw.error} retrying={raw.isRefetching} onRetry={() => void raw.refetch()} /> : <SkeletonRows read={raw} rows={3} />}
    </div>
  );
}

export function RawCallView({ item, raw }: { item: RawItem; raw: RawBody }) {
  const { t } = useTranslation();
  const [search, setSearch] = useState('');
  const input = parse(raw.input.inline);
  const output = raw.output;
  const result = output === null ? null : parse(output.inline);
  return (
    <div className="flex flex-col gap-3" data-raw={item.id}>
      <p className="flex items-center gap-2 text-sm">
        <span className="text-text-muted">{t('timeline.raw.tool')}</span>
        <code className="rounded-sm bg-bg-muted px-1">{raw.name}</code>
      </p>
      <Input label={t('timeline.raw.search')} hideLabel placeholder={t('timeline.raw.search')} value={search} onChange={(event) => setSearch(event.target.value)} fieldClassName="max-w-(--container-form)" />
      <section className="flex flex-col gap-1">
        <h2 className="flex items-center justify-between text-xs font-medium text-text-muted">
          {t('timeline.raw.input')}
          {raw.input.inline !== '' && <CopyButton value={raw.input.inline} label={t(raw.input.truncated ? 'timeline.raw.copyInputShown' : 'timeline.raw.copyInput')} />}
        </h2>
        {raw.input.inline === '' && raw.input.blob !== null ? null : input.ok ? <JsonTree value={input.value} search={search} label={t('timeline.raw.input')} /> : <CodeBlock code={raw.input.inline} />}
        {raw.input.truncated && raw.input.blob === null && <p className="text-xs text-text-subtle">{t('timeline.raw.cut')}</p>}
        {raw.input.blob !== null && <FileAttachment thumbnail={false} source={blobSource('thread', item.thread_id, raw.input.blob, t('timeline.raw.fullInput'), raw.input.total_len, 'text/plain')} />}
      </section>
      <section className="flex flex-col gap-1">
        <h2 className="flex items-center justify-between text-xs font-medium text-text-muted">
          {t('timeline.raw.output')}
          {output !== null && output.inline !== '' && <CopyButton value={output.inline} label={t(output.truncated ? 'timeline.raw.copyOutputShown' : 'timeline.raw.copyOutput')} />}
        </h2>
        {raw.is_error && <InlineAlert tone="danger">{t('timeline.raw.error')}</InlineAlert>}
        {output === null ? (
          <p className="text-sm text-text-muted">{item.status === 'in_progress' ? t('timeline.raw.running') : t('timeline.raw.noOutput')}</p>
        ) : output.inline === '' && output.blob !== null ? null : result?.ok === true && typeof result.value === 'object' && result.value !== null ? (
          <JsonTree value={result.value} search={search} label={t('timeline.raw.output')} />
        ) : (
          <CodeBlock code={output.inline} maxHeight="24rem" />
        )}
        {output?.truncated === true && output.blob === null && <p className="text-xs text-text-subtle">{t('timeline.raw.cut')}</p>}
        {output?.blob !== undefined && output.blob !== null && <FileAttachment thumbnail={false} source={blobSource('thread', item.thread_id, output.blob, t('timeline.raw.fullOutput'), output.total_len, 'text/plain')} />}
      </section>
    </div>
  );
}
