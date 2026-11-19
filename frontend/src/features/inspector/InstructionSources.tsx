/** Instructions this agent reads: Orch's introduction, the guide to its tools, each place's standing instructions and
 * the files its harness reads, in the order the agent reads them, as the last session was opened with them.
 *
 * Each row names its origin, its size, what was taken in and what was cut; an unknown value says so, with its reason.
 * What the harness reported is told apart from what its documented loading rule finds. A file's path is shown only to
 * whoever may view the files of its environment, and a file of a person's home only by its label. A change of the
 * standing instructions that waits for the next turn says so. This is an ordered list, not a claim that one file is
 * all the agent was given.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { threadInstructionsQuery } from '@/api/instructions';
import type { InstructionSource } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { ByteSize } from '@/ui/ByteSize';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { openFile } from '@/features/files/FileTabs';
import { openGiven } from './givenFocus';
import { useViewInstance } from '@/features/workbench/instance';
import { sourceEvidenceWord, sourceKindWord } from './words';

/** What a row is called: a file by its own label (a path or a name), what Orch gave by its kind and its place. */
function title(t: TFunction, source: InstructionSource): string {
  if (source.kind === 'file' || source.kind === 'memory') return source.label;
  const kind = sourceKindWord(t, source.kind);
  const place = source.scope?.name ?? null;
  return place === null ? kind : `${kind} · ${place}`;
}

function Size({ source }: { source: InstructionSource }) {
  const { t } = useTranslation();
  if (source.bytes === null) return <span className="text-text-muted">{t('threadViews.given.sources.unknown')}</span>;
  return <span className="text-text-muted">
    <ByteSize bytes={source.bytes} />
    {' · '}{t('threadViews.given.sources.loaded')}{' '}{source.loaded_bytes === null ? t('threadViews.given.sources.unknown') : <ByteSize bytes={source.loaded_bytes} />}
    {source.cut_bytes !== null && source.cut_bytes > 0 && <> · {t('threadViews.given.sources.cut')} <ByteSize bytes={source.cut_bytes} /></>}
    {source.lines !== null && <> · {t('threadViews.given.sources.lines', { count: source.lines })}</>}
    {typeof source.line_limit === 'number' && <> · {t('threadViews.given.sources.lineLimit', { count: source.line_limit })}</>}
    {typeof source.byte_limit === 'number' && <> · {t('threadViews.given.sources.byteLimit')} <ByteSize bytes={source.byte_limit} /></>}
  </span>;
}

export function InstructionSources({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const owner = useViewInstance()?.id;
  const query = useQuery(threadInstructionsQuery(threadId));
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={2} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const { sources, standing } = query.data;
  return <details className="border-b border-border px-3 py-2" data-testid="instruction-sources">
    <summary className="cursor-pointer text-sm font-medium">
      {t('threadViews.given.sources.title')}
      {standing.pending_next_turn && <Badge tone="warning" className="ms-2">{t('threadViews.given.sources.nextTurn')}</Badge>}
    </summary>
    {sources.length === 0
      ? <EmptyState compact title={t('threadViews.given.sources.none')} />
      : <ol className="mt-2 space-y-1 text-sm" aria-label={t('threadViews.given.sources.title')}>
        {sources.map((source) => <li key={source.order} className="flex flex-col gap-1 rounded-md border border-border px-2 py-1">
          <span className="flex flex-wrap items-center gap-2">
            {source.readable && typeof source.workspace_path === 'string' && source.source_environment !== null
              ? <Button size="sm" variant="ghost" onClick={() => { if (typeof source.workspace_path === 'string' && source.source_environment !== null) openFile({ environmentId: source.source_environment.id, threadId, path: source.workspace_path }); }}>{title(t, source)}</Button>
              : source.readable && (source.kind === 'preamble' || source.kind === 'guide' || source.kind === 'standing')
                ? <Button size="sm" variant="ghost" onClick={() => openGiven(threadId, {}, owner)}>{title(t, source)}</Button>
                : <span className="font-medium">{title(t, source)}</span>}
            <span className="text-xs text-text-muted">{sourceEvidenceWord(t, source.evidence)}</span>
          </span>
          {source.path !== null && <span className="break-all font-mono text-xs text-text-muted">{source.path}</span>}
          {typeof source.parent_path === 'string' && <span className="break-all text-xs text-text-muted">{t('threadViews.given.sources.importedFrom', { path: source.parent_path })}</span>}
          <Size source={source} />
          {source.reason !== null && <span className="text-xs text-text-muted">{source.reason}</span>}
        </li>)}
      </ol>}
    <p className="mt-2 text-xs text-text-muted">{t('threadViews.given.sources.standing', { characters: standing.characters, limit: standing.limit })}</p>
  </details>;
}
