/** A topic selection acts at once: one notice with a revision-guarded Undo for confirmed successes, refusals named by their rows. Delete moves the topics to the trash at once, with one line and its own Undo. */
import { READ_NOW } from '@/api/freshness';
import { useQueryClient } from '@tanstack/react-query';
import { CircleCheck, CircleDot, RotateCcw, Search, Share2, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { changeTopicStatus } from '@/api/day100';
import { ArchivePlaces, MovePlaces } from '@/features/project/PlaceActs';
import type { ReviewAction } from '@/features/project/reviewBatch';
import { topicQuery } from '@/api/nodes';
import { openAccessToMany } from '@/features/access/AccessDialog';
import { invalidatePlaces } from '@/features/navigation/refresh';
import { placeOfNode } from '@/features/navigation/places';
import { usePlaceTrash } from '@/features/trash/usePlaceTrash';
import { confirmedInverses, statusBatch, statusBlockers, type StatusResult } from '@/features/project/selection';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { denial } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SelectedBar } from '@/ui/RowSelection';
import { toast } from '@/ui/Toast';

const STATUSES: readonly TopicStatus[] = ['done', 'open'];
const PLACE_ACTIONS: readonly ReviewAction[] = ['move', 'archive', 'unarchive'];
const STATUS_PERMISSION = 'topic.interact';

export function TopicSelection({ selected, onClear }: { selected: readonly Topic[]; onClear: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [review, setReview] = useState<{ rows: Topic[]; status: TopicStatus } | null>(null);
  const [places, setPlaces] = useState<{ rows: Topic[]; action: ReviewAction } | null>(null);
  const [undoing, setUndoing] = useState(false);
  const [results, setResults] = useState<StatusResult[] | null>(null);
  const [inverses, setInverses] = useState<StatusResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState<string | null>(null);
  const [checkError, setCheckError] = useState<unknown>(null);
  const trash = usePlaceTrash();
  const [deleting, setDeleting] = useState(false);
  // Delete moves the selected topics to the trash at once; its one line says what went and offers Undo.
  const remove = async () => {
    setDeleting(true);
    try {
      await trash(selected.map(placeOfNode));
      onClear();
    } finally { setDeleting(false); }
  };
  const undo = async (done: readonly StatusResult[], quiet = false) => {
    setBusy(true);
    const found: StatusResult[] = [];
    for (const pair of confirmedInverses(done)) {
      const result = await statusBatch([pair.after], pair.before.status, (row, status) => changeTopicStatus(row.id, status, row.revision), (one) => { if (!quiet) setInverses([...found, one]); });
      found.push(...result);
    }
    if (!quiet) setInverses(found);
    else if (found.some((one) => one.state !== 'succeeded')) toast({ tone: 'danger', title: t('placeReview.undoChanged') });
    setBusy(false);
  };
  // The chosen status is written at once; only rows that did not succeed open the results.
  const changeStatus = async (rows: Topic[], status: TopicStatus) => {
    setBusy(true);
    const done = await statusBatch(rows, status, (row, next) => changeTopicStatus(row.id, next, row.revision), () => undefined);
    setBusy(false);
    onClear();
    const succeeded = done.filter((one) => one.state === 'succeeded').length;
    if (succeeded > 0) toast({ tone: 'success', title: t(status === 'done' ? 'day100.batch.markedDone' : 'day100.batch.reopened', { count: succeeded }),
      action: { label: t('day100.batch.undo'), run: () => { setUndoing(true); void undo(done, true).finally(() => setUndoing(false)); } } });
    if (done.some((one) => one.state !== 'succeeded')) { setReview({ rows, status }); setResults(done); }
  };
  const check = async (row: Topic) => {
    setReading(row.id);
    setCheckError(null);
    try {
      const current = await client.query({ ...topicQuery(row.id), ...READ_NOW });
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
      {places?.action === 'move' && <MovePlaces rows={places.rows} onClose={() => setPlaces(null)} onApplied={onClear} />}
      {(places?.action === 'archive' || places?.action === 'unarchive') && <ArchivePlaces rows={places.rows} archiving={places.action === 'archive'} onClose={() => setPlaces(null)} onApplied={onClear} />}
      {selected.length > 0 && <div role="region" aria-label={t('day100.batch.selection')} className="sticky top-0 z-(--layer-sticky) flex flex-col gap-1 bg-bg">
        <SelectedBar count={selected.length} busy={deleting} onClear={onClear}>
          <Button size="sm" icon={<Share2 aria-hidden />} onClick={() => openAccessToMany(selected.map((topic) => ({kind:'topic',id:topic.id,name:topic.name})))}>{t('access.menu.share')}</Button>
          {PLACE_ACTIONS.map((action) => <Button key={action} size="sm" blocked={deleting} onClick={() => setPlaces({ rows: [...selected], action })}>{t(action === 'move' ? 'actions.move.label' : action === 'archive' ? 'actions.archive.label' : 'places.menu.unarchive')}</Button>)}
          {STATUSES.map((status) => <Button key={status} size="sm" icon={status === 'done' ? <CircleCheck aria-hidden /> : <CircleDot aria-hidden />} disabled={blockers(status).length > 0 || busy || undoing || deleting} onClick={() => void changeStatus([...selected], status)}>{t(status === 'done' ? 'places.menu.markDone' : 'places.menu.reopen')}</Button>)}
          <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} pending={deleting} onClick={() => void remove()}>{t('actions.delete.label')}</Button>
        </SelectedBar>
        {STATUSES.filter((status) => blockers(status).some((one) => one.reason !== 'already')).map((status) => <p key={status} className="text-sm text-text-muted">{blockers(status).filter((one) => one.reason !== 'already').map(({ topic, reason }) => reason === 'permission' ? denial(t, STATUS_PERMISSION, topic.name, topic.owner) : t('day100.batch.archived', { name: topic.name })).join(' ')}</p>)}
      </div>}
      <Dialog open={open && results !== null} dismissible={!busy} onOpenChange={(on) => { if (!on) close(); }} title={t('day100.batch.results', { count: review?.rows.length ?? results?.length ?? 0 })} footer={<>
        {results !== null && confirmedInverses(results).length > 0 && inverses === null && <Button icon={<RotateCcw aria-hidden />} pending={busy} onClick={() => void undo(results)}>{t('day100.batch.undo')}</Button>}
      </>}>
        <div className="flex flex-col gap-3" aria-live="polite">
          {checkError !== null && <ErrorAlert error={checkError} />}
          <ul className="flex flex-col gap-2">{results?.map((result) => <li key={result.before.id} className="flex flex-col gap-2 rounded-md border border-border p-3">
            <p className="flex flex-wrap items-center justify-between gap-2"><span className="font-medium wrap-anywhere">{result.before.name}</span><span className="text-sm text-text-muted">{t(`day100.batch.states.${result.state}`)}</span></p>
            {result.error !== undefined && <ErrorAlert error={result.error} />}
            {result.state === 'uncertain' && <InlineAlert tone="warning">{t('day100.batch.uncertain')}</InlineAlert>}
            {(result.state === 'uncertain' || result.state === 'refused') && <><Button size="sm" icon={<Search aria-hidden />} pending={reading === result.before.id} onClick={() => void check(result.before)}>{t('day100.batch.check')}</Button>{result.observed !== undefined && <p className="text-sm text-text-muted">{t('day100.batch.observed', { status: t(`places.topic.statuses.${result.observed.status}`) })}</p>}</>}
          </li>)}</ul>
          {inverses !== null && <section className="flex flex-col gap-2"><h3 className="font-medium">{t('day100.batch.undoResults')}</h3><ul className="flex flex-col gap-2">{inverses.map((result) => <li key={result.before.id} className="rounded-md border border-border p-3"><p className="font-medium">{result.before.name} · {t(`day100.batch.states.${result.state}`)}</p>{result.error !== undefined && <ErrorAlert error={result.error} />}</li>)}</ul></section>}
        </div>
      </Dialog>
    </>
  );
}
