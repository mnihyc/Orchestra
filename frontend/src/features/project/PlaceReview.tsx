/** One complete selected-place review, sequential results, explicit reconciliation and one successful-row Undo. */
import { useQueries } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { previewArchive, previewDelete, previewMove, readNode } from '@/api/nodes';
import { useNodeLabel, usePlaceLabels } from '@/features/navigation/placeLabels';
import { ArchiveConsequences, DeleteConsequences, MoveConsequences } from '@/features/project/ReviewConsequences';
import { applyRows, canUndo, undoPlace, type PlacePlan, type ReviewAction, type RowResult } from '@/features/project/reviewBatch';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { Node } from '@/generated/api-types';
import { placedOfPath } from '@/lib/placeLabel';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

function PlanBody({ review }: { review: PlacePlan }) {
  switch (review.kind) {
    case 'archive': case 'unarchive': return <ArchiveConsequences plan={review.plan} />;
    case 'move': return <MoveConsequences plan={review.plan} />;
    case 'delete': return <DeleteConsequences plan={review.plan} />;
  }
}

function Observation({ result }: { result: RowResult }) {
  const { t } = useTranslation();
  const current = result.observed;
  const parent = current?.path.at(-2);
  const parentName = useNodeLabel(parent?.id ?? '', parent?.redacted === true ? undefined : parent?.name ?? undefined);
  if (current === undefined) return null;
  if (result.review.kind === 'delete' && current.inspection !== undefined && current.inspection !== null) return <p>{t('placeReview.observedDeleted', { revision: current.revision })}</p>;
  return <p>{result.review.kind === 'move' ? t('placeReview.observedParent', { parent: parentName ?? t('placeReview.private'), revision: current.revision }) : t('placeReview.observedArchive', { state: current.archived ? t('placeReview.readOnly') : t('placeReview.ordinaryRights'), revision: current.revision })}</p>;
}

export function PlaceReview({ rows, action, onClose, onApplied }: { rows: readonly Node[]; action: ReviewAction; onClose: () => void; onApplied: () => void }) {
  const { t } = useTranslation();
  const [destination, setDestination] = useState<PickedPlace | null>(null);
  const [generation, setGeneration] = useState(0);
  const [active, setActive] = useState(rows);
  const [kept, setKept] = useState<RowResult[]>([]);
  const [results, setResults] = useState<RowResult[] | null>(null);
  const [inverses, setInverses] = useState<{ id: string; error?: unknown }[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [reading, setReading] = useState<string | null>(null);
  const [readError, setReadError] = useState<unknown>(null);
  const [keys, setKeys] = useState(() => new Map(rows.map((row) => [row.id, newActKey()])));
  const placed = useMemo(() => rows.flatMap((row) => { const place = placedOfPath(row.path); return place === null ? [] : [place]; }), [rows]);
  const labels = usePlaceLabels(placed);
  const query = useQueries({ queries: active.map((row) => ({
    queryKey: ['place-review', action, row.id, destination?.id ?? '', generation],
    queryFn: async (): Promise<PlacePlan> => {
      const before = await readNode(row.id);
      if (action === 'archive' || action === 'unarchive') return { kind: action, before, plan: await previewArchive(row.id, action === 'archive') };
      if (action === 'delete') return { kind: action, before, plan: await previewDelete(row.id) };
      if (destination === null || before.kind === 'thread') throw new Error(t('placeReview.destinationFirst'));
      return { kind: 'move', before, plan: await previewMove(before.kind, before.id, destination.id) };
    },
    enabled: action !== 'move' || destination !== null, retry: false, staleTime: 0,
  })) });
  const plans = query.flatMap((entry) => entry.isError || entry.data === undefined ? [] : [entry.data]);
  const ready = plans.filter((plan) => plan.kind === 'delete' || plan.plan.blockers.length === 0);
  const unplanned = active.filter((_row, index) => query[index]?.isError === true);
  const loaded = query.every((entry) => !entry.isPending && !entry.isFetching);
  const combined = [...kept, ...(results ?? [])];
  const update = (result: RowResult) => {
    setKept((before) => before.map((row) => row.review.before.id === result.review.before.id ? result : row));
    setResults((before) => (before ?? []).map((row) => row.review.before.id === result.review.before.id ? result : row));
  };
  const confirm = async () => {
    setBusy(true); setResults(plans.map((review) => ({ review, state: 'queued' })));
    try { setResults(await applyRows(plans, keys, update)); onApplied(); }
    finally { setBusy(false); }
  };
  const undo = async () => {
    setBusy(true);
    const output: { id: string; error?: unknown }[] = [];
    try { for (const result of combined.filter(canUndo)) {
      try { await undoPlace(result, t('placeReview.undoChanged')); output.push({ id: result.review.before.id }); update({ ...result, undone: true }); }
      catch (error) { output.push({ id: result.review.before.id, error }); }
      setInverses([...output]);
    } } finally { setBusy(false); }
  };
  const reconcile = async (result: RowResult) => {
    setReading(result.review.before.id); setReadError(null);
    try { const observed = await readNode(result.review.before.id, result.review.kind === 'delete'); update({ ...result, state: result.state === 'succeeded' ? 'succeeded' : 'observed', observed }); }
    catch (error) { setReadError(error); } finally { setReading(null); }
  };
  const again = () => {
    const previous = combined.filter((row) => row.state === 'succeeded' || row.state === 'uncertain');
    const retry = [...unplanned, ...combined.filter((row) => row.state === 'refused' || row.state === 'skipped' || row.state === 'observed').map((row) => row.review.before)];
    setKept(previous); setActive(retry); setResults(null); setGeneration((value) => value + 1); setKeys(new Map(retry.map((row) => [row.id, newActKey()])));
  };
  return <Dialog open size="lg" dismissible={!busy} onOpenChange={(open) => { if (!open && !busy) onClose(); }} title={t(results === null ? 'placeReview.title' : 'placeReview.resultTitle')} footer={<>
    <Button disabled={busy} onClick={onClose}>{t('common.close')}</Button>
    {results === null ? <Button variant={action === 'delete' ? 'danger' : 'primary'} disabled={!loaded || ready.length === 0} pending={busy} onClick={() => void confirm()}>{t('placeReview.apply', { count: ready.length })}</Button> : <>
      {combined.some(canUndo) && <Button pending={busy} onClick={() => void undo()}>{t('placeReview.undo')}</Button>}
      {(unplanned.length > 0 || results.some((row) => row.state === 'refused' || row.state === 'skipped' || row.state === 'observed')) && <Button disabled={busy} onClick={again}>{t('placeReview.reviewAgain')}</Button>}
    </>}
  </>}>
    {results === null ? <div className="flex flex-col gap-4">
      <p>{t('placeReview.selected', { count: active.length })}</p>
      {action === 'move' && <PlacePicker label={t('placeReview.chooseDestination')} value={destination} onChange={setDestination} only="project" rootOrgId={rows[0]?.org_id} />}
      {action === 'move' && destination === null && <p>{t('placeReview.destinationFirst')}</p>}
      <ul className="flex flex-col gap-4">{active.map((row, index) => { const item = query[index]; return <li key={row.id} className="flex flex-col gap-3 rounded-md border border-border p-3"><h3 className="font-medium wrap-anywhere">{labels.get(row.id) ?? row.name}</h3>{item === undefined || item.isPending ? <SkeletonRows rows={3} /> : item.isError ? <ErrorState compact error={item.error} onRetry={() => void item.refetch()} /> : item.isFetching ? <SkeletonRows rows={3} /> : <PlanBody review={item.data} />}</li>; })}</ul>
    </div> : <div className="flex flex-col gap-4" aria-live="polite">
      {(unplanned.length > 0 || combined.some((row) => row.state !== 'succeeded')) && <InlineAlert tone="warning">{t('placeReview.partial')}</InlineAlert>}
      {readError !== null && <ErrorAlert error={readError} />}
      <ul className="flex flex-col gap-3">{unplanned.map((row) => <li key={row.id} className="rounded-md border border-border p-3"><h3>{labels.get(row.id) ?? row.name}</h3><p>{t('placeReview.skipped')}</p>{query[active.indexOf(row)]?.error !== null && <ErrorAlert error={query[active.indexOf(row)]?.error} />}</li>)}{combined.map((row) => <li key={row.review.before.id} className="flex flex-col gap-2 rounded-md border border-border p-3"><h3 className="font-medium wrap-anywhere">{labels.get(row.review.before.id) ?? row.review.before.name}</h3><p>{t(`placeReview.${row.state}`)}</p>{row.undone === true && <p>{t('placeReview.undone')}</p>}{row.state === 'skipped' && <PlanBody review={row.review} />}{row.error !== undefined && <ErrorAlert error={row.error} />}{row.state === 'uncertain' && <InlineAlert tone="warning">{t('placeReview.uncertainAbout')}</InlineAlert>}{(row.state === 'uncertain' || row.state === 'refused' || row.state === 'observed') && <Button size="sm" pending={reading === row.review.before.id} onClick={() => void reconcile(row)}>{t('placeReview.readAgain')}</Button>}<Observation result={row} /></li>)}</ul>
      {inverses !== null && <section aria-label={t('placeReview.undoTitle')}><h3>{t('placeReview.undoTitle')}</h3><ul className="flex flex-col gap-2">{inverses.map((result) => <li key={result.id}><p>{labels.get(result.id) ?? result.id}</p>{result.error === undefined ? <p>{t('placeReview.succeeded')}</p> : <><ErrorAlert error={result.error} /><Button size="sm" pending={reading === result.id} onClick={() => { const row = combined.find((row) => row.review.before.id === result.id); if (row !== undefined) void reconcile(row); }}>{t('placeReview.readAgain')}</Button></>}</li>)}</ul></section>}
    </div>}
  </Dialog>;
}
