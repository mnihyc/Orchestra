/** A topic selection has one frozen combined review, per-row results and one revision-guarded Undo for confirmed successes. */
import { useQueryClient } from '@tanstack/react-query';
import { CircleCheck, CircleDot, RotateCcw, Search, Share2, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { changeTopicStatus } from '@/api/day100';
import { PlaceReview } from '@/features/project/PlaceReview';
import type { ReviewAction } from '@/features/project/reviewBatch';
import { topicQuery } from '@/api/nodes';
import { openAccessToMany } from '@/features/access/AccessDialog';
import { invalidatePlaces } from '@/features/navigation/refresh';
import { confirmedInverses, statusBatch, statusBlockers, type StatusResult } from '@/features/project/selection';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { denial } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { StateChip } from '@/ui/StateChip';

const STATUSES: readonly TopicStatus[] = ['done', 'open'];
const PLACE_ACTIONS: readonly ReviewAction[] = ['move', 'archive', 'unarchive', 'delete'];
const STATUS_PERMISSION = 'topic.interact';

export function TopicSelection({ selected, onClear }: { selected: readonly Topic[]; onClear: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [review, setReview] = useState<{ rows: Topic[]; status: TopicStatus } | null>(null);
  const [places, setPlaces] = useState<{ rows: Topic[]; action: ReviewAction } | null>(null);
  const [results, setResults] = useState<StatusResult[] | null>(null);
  const [inverses, setInverses] = useState<StatusResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState<string | null>(null);
  const [checkError, setCheckError] = useState<unknown>(null);
  const update = (result: StatusResult) => setResults((before) => before?.some((one) => one.before.id === result.before.id) ? before.map((one) => one.before.id === result.before.id ? result : one) : [...before ?? [], result]);
  const confirm = async () => {
    if (review === null) return;
    setBusy(true);
    setResults(review.rows.map((before) => ({ before, state: 'queued' })));
    const done = await statusBatch(review.rows, review.status, (row, status) => changeTopicStatus(row.id, status, row.revision), update);
    setResults(done);
    setBusy(false);
    onClear();
  };
  const undo = async () => {
    setBusy(true);
    const found: StatusResult[] = [];
    for (const pair of confirmedInverses(results ?? [])) {
      const result = await statusBatch([pair.after], pair.before.status, (row, status) => changeTopicStatus(row.id, status, row.revision), (one) => { setInverses([...found, one]); });
      found.push(...result);
    }
    setInverses(found);
    setBusy(false);
  };
  const check = async (row: Topic) => {
    setReading(row.id);
    setCheckError(null);
    try {
      const current = await client.query({ ...topicQuery(row.id), staleTime: 0 });
      // A write whose reply was lost may have happened: this explicit check also rereads its related lists.
      void invalidatePlaces(client, current.org_id);
      setResults((before) => before?.map((result) => result.before.id === row.id ? { ...result, observed: current } : result) ?? null);
    } catch (error) { setCheckError(error); } finally { setReading(null); }
  };
  const open = review !== null || results !== null;
  const blockers = (status: TopicStatus) => statusBlockers(selected, status);
  const close = () => { if (!busy) { setReview(null); setResults(null); setInverses(null); } };
  return (
    <>
      {places !== null && <PlaceReview rows={places.rows} action={places.action} onClose={() => setPlaces(null)} onApplied={onClear} />}
      {selected.length > 0 && <div role="region" className="flex flex-wrap items-center gap-2 rounded-lg border border-accent/40 bg-accent-subtle p-3" aria-label={t('day100.batch.selection')}>
        <span className="text-sm font-medium text-text">{t('day100.batch.selected', { count: selected.length })}</span>
        <Button size="sm" icon={<Share2 aria-hidden />} onClick={() => openAccessToMany(selected.map((topic) => ({kind:'topic',id:topic.id,name:topic.name})))}>{t('access.menu.share')}</Button>
        {PLACE_ACTIONS.map((action) => <Button key={action} size="sm" onClick={() => setPlaces({ rows: [...selected], action })}>{t(action === 'move' ? 'actions.move.label' : action === 'archive' ? 'actions.archive.label' : action === 'unarchive' ? 'places.menu.unarchive' : 'actions.delete.label')}</Button>)}
        {STATUSES.map((status) => <Button key={status} size="sm" icon={status === 'done' ? <CircleCheck aria-hidden /> : <CircleDot aria-hidden />} disabled={blockers(status).length > 0} onClick={() => setReview({ rows: [...selected], status })}>{t(status === 'done' ? 'places.menu.markDone' : 'places.menu.reopen')}</Button>)}
        <Button size="sm" variant="ghost" icon={<X aria-hidden />} onClick={onClear}>{t('day100.batch.clear')}</Button>
        {STATUSES.filter((status) => blockers(status).some((one) => one.reason !== 'already')).map((status) => <p key={status} className="w-full text-sm text-text-muted">{blockers(status).filter((one) => one.reason !== 'already').map(({ topic, reason }) => reason === 'permission' ? denial(t, STATUS_PERMISSION, topic.name, topic.owner) : t('day100.batch.archived', { name: topic.name })).join(' ')}</p>)}
      </div>}
      <Dialog open={open} dismissible={!busy} onOpenChange={(on) => { if (!on) close(); }} title={t(results === null ? 'day100.batch.review' : 'day100.batch.results', { count: review?.rows.length ?? results?.length ?? 0 })} footer={<>
        {results === null && review !== null && <Button variant="primary" icon={review.status === 'done' ? <CircleCheck aria-hidden /> : <CircleDot aria-hidden />} pending={busy} onClick={() => void confirm()}>{t(review.status === 'done' ? 'day100.batch.markDone' : 'day100.batch.reopen', { count: review.rows.length })}</Button>}
        {results !== null && confirmedInverses(results).length > 0 && inverses === null && <Button icon={<RotateCcw aria-hidden />} pending={busy} onClick={() => void undo()}>{t('day100.batch.undo')}</Button>}
      </>}>
        {results === null && review !== null ? <div className="flex flex-col gap-3">
          <p className="text-sm text-text-muted">{t('day100.batch.consequences')}</p>
          <ul className="flex flex-col gap-2">{review.rows.map((row) => <li key={row.id} className="flex flex-wrap items-center gap-2 rounded-md border border-border p-3"><span className="min-w-0 flex-1 font-medium wrap-anywhere">{row.name}</span><StateChip state={row.status === 'done' ? 'done' : 'idle'} label={t(`places.topic.statuses.${row.status}`)} /><span aria-hidden>→</span><StateChip state={review.status === 'done' ? 'done' : 'idle'} label={t(`places.topic.statuses.${review.status}`)} /></li>)}</ul>
        </div> : <div className="flex flex-col gap-3" aria-live="polite">
          {checkError !== null && <ErrorAlert error={checkError} />}
          <ul className="flex flex-col gap-2">{results?.map((result) => <li key={result.before.id} className="flex flex-col gap-2 rounded-md border border-border p-3">
            <p className="flex flex-wrap items-center justify-between gap-2"><span className="font-medium wrap-anywhere">{result.before.name}</span><span className="text-sm text-text-muted">{t(`day100.batch.states.${result.state}`)}</span></p>
            {result.error !== undefined && <ErrorAlert error={result.error} />}
            {result.state === 'uncertain' && <InlineAlert tone="warning">{t('day100.batch.uncertain')}</InlineAlert>}
            {(result.state === 'uncertain' || result.state === 'refused') && <><Button size="sm" icon={<Search aria-hidden />} pending={reading === result.before.id} onClick={() => void check(result.before)}>{t('day100.batch.check')}</Button>{result.observed !== undefined && <p className="text-sm text-text-muted">{t('day100.batch.observed', { status: t(`places.topic.statuses.${result.observed.status}`) })}</p>}</>}
          </li>)}</ul>
          {inverses !== null && <section className="flex flex-col gap-2"><h3 className="font-medium">{t('day100.batch.undoResults')}</h3><ul className="flex flex-col gap-2">{inverses.map((result) => <li key={result.before.id} className="rounded-md border border-border p-3"><p className="font-medium">{result.before.name} · {t(`day100.batch.states.${result.state}`)}</p>{result.error !== undefined && <ErrorAlert error={result.error} />}</li>)}</ul></section>}
        </div>}
      </Dialog>
    </>
  );
}
