/** One mixed restore confirmation keeps individual refusals, included descendants and durable environment jobs visible. */
import { useQuery } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { Job, Node, AuditEntry as TraceEntry } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { JobProgress } from './KeptEnvironments';
import { refusalJob, reviewRestores, runRestores, selectionKey, type RestoreResult } from './restore';

export function RestoreDialog({ selected, refresh, onClose, onIncludeParent }: { selected: TraceEntry[]; refresh: () => Promise<TraceEntry[]>; onClose: () => void; onIncludeParent: (entry: TraceEntry) => void }) {
  const { t } = useTranslation(); const format = useFormatContext(); const [reviewKey] = useState(newActKey);
  const review = useQuery({ queryKey: ['trace-restore-review', reviewKey], queryFn: () => reviewRestores(selected, refresh), retry: false, staleTime: Infinity, gcTime: 0 });
  const [results, setResults] = useState<Record<string, RestoreResult>>({}); const [working, setWorking] = useState(false); const [sent, setSent] = useState(false);
  const eligible = review.data?.filter((row) => row.block === undefined && row.includedIn === undefined) ?? [];
  const confirm = async () => {
    if (working || sent || eligible.length === 0) return;
    setWorking(true); setSent(true);
    await runRestores(review.data ?? [], (key, result) => setResults((current) => ({ ...current, [key]: result })));
    setWorking(false);
  };
  const date = (value: string | null | undefined) => value ? new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(value)) : t('trace.notRecorded');
  return <Dialog open title={t(sent ? 'trace.restoreResults' : 'trace.restoreTitle')} description={t('trace.restoreAbout')} onOpenChange={(open) => !open && !working && onClose()} dismissible={!working} footer={sent ? <Button onClick={onClose} disabled={working}>{t('trace.closeResults')}</Button> : <><Button onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" icon={<RotateCcw aria-hidden />} blocked={review.isPending || review.isError || eligible.length === 0} pending={working} onClick={() => void confirm()}>{t('trace.bringBack')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-3 py-2">
      {review.isPending ? <SkeletonRows rows={3} /> : review.isError ? <ErrorState compact error={review.error} onRetry={() => void review.refetch()} /> : <ul className="min-w-0 divide-y divide-border rounded-lg border border-border">{review.data.map((row) => {
        const entry = row.entry; const key = selectionKey(entry); const root = row.includedIn === undefined ? undefined : review.data.find((candidate) => selectionKey(candidate.entry) === row.includedIn);
        const result = results[row.includedIn ?? key]; const name = entry.object.name ?? t('trace.unknown');
        const restored = entry.object.kind === 'document' ? t('trace.restoredDocument', { name }) : (result?.value as Node | undefined)?.archived === true ? t('trace.restoredArchived', { name }) : t('trace.restored', { name });
        const competingJob = refusalJob(result?.error);
        const parentCandidate = row.parentCandidate;
        return <li key={key} data-restore-result={entry.object.id ?? undefined} className="flex min-w-0 flex-col gap-2 p-3">
          <div className="flex flex-wrap gap-x-2"><strong className="wrap-anywhere">{name}</strong><span className="text-sm text-text-muted">{t(`trace.objectKinds.${entry.object.kind}`, { defaultValue: entry.object.kind })}</span></div>
          <p className="text-sm wrap-anywhere text-text-muted">{entry.object.deleted_by ? t('trace.deletedBy', { name: entry.object.deleted_by.display_name, day: date(entry.object.deleted_at), until: date(entry.object.purge_at) }) : t('trace.deletedAnonymous', { day: date(entry.object.deleted_at), until: date(entry.object.purge_at) })}</p>
          <p className="text-sm wrap-anywhere">{t(entry.object.kind === 'environment' ? 'trace.returnsStopped' : entry.object.kind === 'document' ? 'trace.returnsDocument' : 'trace.returnsArchived')}</p>
          {row.block !== undefined && <p role="status" className="text-sm text-warning-text">{t(`trace.restoreBlocks.${row.block}`, { name: row.parent?.name ?? t('trace.unknown') })}</p>}
          {!sent && parentCandidate !== undefined && <Button size="sm" icon={<RotateCcw aria-hidden />} onClick={() => onIncludeParent(parentCandidate)}>{t('trace.includeParent', { name: parentCandidate.object.name ?? t('trace.unknown') })}</Button>}
          {row.includedIn !== undefined && <p className="text-sm text-text-muted">{t('trace.includedWith', { name: root?.entry.object.name ?? t('trace.unknown') })}</p>}
          {row.parentUnconfirmed && <p className="text-sm text-text-muted">{t('trace.parentCheck')}</p>}
          {row.error !== undefined && <ErrorAlert error={row.error} />}
          {result?.state === 'pending' && <p role="status" className="text-sm">{t('trace.restoringSelection')}</p>}
          {result?.state === 'restored' && <p role="status" className="text-sm text-success-text">{row.includedIn !== undefined ? t('trace.restoredWith', { name: root?.entry.object.name }) : restored}</p>}
          {result?.state === 'failed' && <ErrorAlert error={result.error} />}
          {result?.state === 'failed' && entry.object.kind === 'environment' && competingJob !== null && <JobProgress id={competingJob} environmentId={entry.object.id ?? ''} />}
          {result?.state === 'job' && entry.object.kind === 'environment' && <JobProgress id={(result.value as Job).id} environmentId={entry.object.id ?? ''} />}
        </li>;
      })}</ul>}
    </div>
  </Dialog>;
}
