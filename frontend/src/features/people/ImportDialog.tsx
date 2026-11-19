/** One import wizard plans a source, follows its job and opens the retained report. */
import { keepPreviousData, useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { client } from '@/api/client';
import { exportKeys } from '@/api/exports';
import { importKeys, planImport, startImport } from '@/api/imports';
import { PLAN_PAUSE_MS } from '@/features/export/plan';
import { ImportDestination, type ImportDestinationValue, useImportDestinationStanding } from '@/features/people/ImportDestination';
import type { ImportTarget } from '@/features/people/importHostState';
import { watchImport } from '@/features/people/importJobs';
import { ImportProgress } from '@/features/people/ImportProgress';
import { ImportReport } from '@/features/people/ImportReport';
import { ImportReview } from '@/features/people/ImportReview';
import { ImportSourceStep } from '@/features/people/ImportSourceStep';
import { ImportSteps, type ImportStep } from '@/features/people/ImportSteps';
import type { ImportSource as Selection } from '@/features/people/importSource';
import { ImportUploads, type ImportUploadProgress } from '@/features/people/importUploads';
import type { ImportAction, ImportChoices, ImportOptions, ImportRequest, ImportSource as UploadedSource, ImportSourceFacts } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatBytes, formatNumber } from '@/lib/format';
import { NetworkError, newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

const PLAN_VIEW = {};

function NewImport({ target, onClose }: { target: Extract<ImportTarget, { kind: 'new' }>; onClose: () => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const [step, setStep] = useState<ImportStep>('source');
  const [selection, setSelection] = useState<Selection | null>(null);
  const [batch, setBatch] = useState<ImportUploads | null>(null);
  const [source, setSource] = useState<UploadedSource | null>(null);
  const [facts, setFacts] = useState<ImportSourceFacts | null>(null);
  const [destination, setDestination] = useState<ImportDestinationValue>({ place: target.place, folder: target.folder, root: 'inside' });
  const [options, setOptions] = useState<ImportOptions>({});
  const [choices, setChoices] = useState<ImportChoices>({});
  const [identity, setIdentity] = useState(newActKey);
  const [uploading, setUploading] = useState(false);
  const [reading, setReading] = useState(false);
  const [closing, setClosing] = useState(false);
  const [progress, setProgress] = useState<ImportUploadProgress | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [notify, setNotify] = useState(false);
  const [jobId, setJobId] = useState<string | null>(null);
  const [reportId, setReportId] = useState<string | null>(null);
  const handed = useRef(false);
  const progressAt = useRef(-Infinity);
  const alive = useRef(true);
  const owner = useRef({ user: client.session.getState().userId, reader: client.session.getState().readerVersion ?? 0 });
  const owns = () => {
    const session = client.session.getState();
    return alive.current && session.userId === owner.current.user && (session.readerVersion ?? 0) === owner.current.reader;
  };
  const sourceReading = useCallback((value: boolean) => setReading(value), []);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => () => { if (!handed.current && batch !== null) void batch.cancel().catch(() => undefined); }, [batch]);

  const currentDestination = step === 'source' ? { ...destination, place: null, folder: null } : destination;
  const standing = useImportDestinationStanding(currentDestination, facts?.mode ?? null);
  const request = useMemo<ImportRequest | null>(() => source === null || destination.place === null ? null : {
    client_id: identity, source, destination: { ...(destination.folder === null ? {} : { folder: destination.folder.id }), root: destination.root }, options, choices,
  }, [source, destination.place, destination.folder, destination.root, identity, options, choices]);
  const snapshot = useMemo(() => request === null || destination.place === null ? null : JSON.stringify({ node: destination.place.id, request }), [destination.place, request]);
  const settled = useDebounced(snapshot, PLAN_PAUSE_MS);
  const planned = useMemo(() => settled === null ? null : JSON.parse(settled) as { node: string; request: ImportRequest }, [settled]);
  const waiting = snapshot !== settled;
  const inspection = useQuery({
    queryKey: importKeys.inspection(planned?.node ?? null, planned?.request ?? null),
    queryFn: ({ signal }) => {
      if (planned === null) throw new Error('Import source missing');
      return planImport(planned.node, planned.request, {}, signal);
    },
    enabled: step === 'destination' && planned !== null && !waiting, retry: false, staleTime: Infinity, gcTime: 0,
  });
  // Retain the external preflight's source fact when going back to Source, and across a new plan for its root choice.
  // eslint-disable-next-line react-hooks/set-state-in-effect
  useEffect(() => { if (inspection.data !== undefined && !waiting) setFacts(inspection.data.source); }, [inspection.data, waiting]);
  const plan = useInfiniteQuery({
    queryKey: importKeys.plan(planned?.node ?? null, planned?.request ?? null, PLAN_VIEW),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => {
      if (planned === null) throw new Error('Import source missing');
      return planImport(planned.node, planned.request, pageParam === null ? {} : { cursor: pageParam }, signal);
    },
    getNextPageParam: (last) => last.objects.next_cursor,
    maxPages: 1,
    enabled: step === 'review' && planned !== null && !waiting,
    // Tabs and filters read this same bounded page. Only changed choices or an explicit page/retry asks again.
    placeholderData: keepPreviousData, retry: false, staleTime: Infinity, gcTime: 0,
  });
  const latest = plan.data?.pages[0];
  const planBusy = waiting || plan.isFetching || plan.isPlaceholderData;
  const start = useMutation({
    mutationFn: async () => {
      if (!owns()) throw new Error('Import reader changed');
      if (request === null || destination.place === null) throw new Error('Import destination missing');
      return startImport(destination.place.id, request);
    },
    onSuccess: (job) => {
      handed.current = true;
      if (!owns()) return;
      cache.setQueryData(exportKeys.job(job.id), job);
      watchImport(job, destination.place?.id ?? null, notify);
      void cache.invalidateQueries({ queryKey: importKeys.mine(), exact: true });
      setJobId(job.id);
      setStep('run');
    },
  });
  const uncertain = start.error instanceof NetworkError;

  const setSourceSelection = async (next: Selection) => {
    if (!owns() || uploading || closing) return;
    setClosing(true); setError(null);
    try {
      await batch?.cancel();
      if (!owns()) return;
      setSelection(next); setBatch(next.fault === null ? new ImportUploads(next) : null);
      setSource(null); setFacts(null); setProgress(null); setOptions({}); setChoices({}); setIdentity(newActKey());
    } catch (failure) { if (owns()) setError(failure); }
    finally { if (owns()) setClosing(false); }
  };
  const upload = async () => {
    if (!owns() || batch === null || uploading || reading || closing) return;
    setUploading(true); setError(null);
    try {
      const ready = await batch.run(new AbortController().signal, (next) => {
        const now = performance.now();
        // Progress remains measured on every chunk; the screen draws at most ten updates a second, plus completion.
        if (owns() && (now - progressAt.current >= 100 || next.completed === next.files)) { progressAt.current = now; setProgress(next); }
      });
      if (!owns()) return;
      setSource({ files: ready.map(({ path, blob }) => ({ name: path, blob: blob.blob })) });
      setIdentity(newActKey());
      setStep('destination');
    } catch (failure) { if (owns()) setError(failure); }
    finally { if (owns()) setUploading(false); }
  };
  const close = async () => {
    if (!owns() || closing || start.isPending) return;
    if (handed.current) { onClose(); return; }
    setClosing(true); setError(null);
    try { await batch?.cancel(); if (owns()) onClose(); }
    catch (failure) { if (owns()) { setError(failure); setClosing(false); } }
  };
  const changeDestination = (next: ImportDestinationValue) => {
    if (next.place?.id !== destination.place?.id) setOptions((current) => ({ ...current, authors: undefined }));
    setDestination(next); setIdentity(newActKey());
  };
  const review = () => {
    if (options.authors === undefined && inspection.data !== undefined) {
      setOptions((current) => ({ ...current, authors: inspection.data.authors }));
      setIdentity(newActKey());
    }
    setStep('review');
  };
  const changeOptions = (patch: Partial<ImportOptions>) => { setOptions((current) => ({ ...current, ...patch })); setIdentity(newActKey()); };
  const changeObject = (sourceId: string, action: ImportAction) => {
    setChoices((current) => ({ ...current, objects: [...(current.objects ?? []).filter((one) => one.source !== sourceId), { source: sourceId, action }] }));
    setIdentity(newActKey());
  };
  const changePerson = (principal: string, user: string | null) => {
    setChoices((current) => ({ ...current, people: [...(current.people ?? []).filter((one) => one.principal !== principal), { principal, user }] }));
    setIdentity(newActKey());
  };
  const openReport = useCallback((id: string) => { setReportId(id); setStep('report'); }, []);
  const canSource = selection?.fault === null && batch !== null && !uploading && !reading && !closing;
  const canDestination = standing.allowed && !waiting && inspection.isSuccess && !inspection.isFetching && !closing;
  const canImport = request !== null && latest?.blockers.length === 0 && !planBusy && !plan.isError && !start.isPending && !closing;
  const placePath = standing.node?.path.map((part) => part.redacted ? t('ui.nameUnknown') : part.name ?? t('ui.nameUnknown')).join(' / ');
  const title = destination.place === null || step !== 'source' && standing.node === undefined ? t('importWizard.dialog.title')
    : t('importWizard.dialog.titleInto', { place: placePath === undefined || placePath === '' ? destination.place.label : placePath });
  const reason = step === 'source' && !canSource && !uploading && !reading && !closing ? t('importWizard.dialog.chooseSource')
    : step === 'destination' && destination.place === null ? t('importWizard.dialog.choosePlace')
    : step === 'review' && latest !== undefined && latest.blockers.length > 0 ? t('importWizard.dialog.blocked') : null;
  const uploadPercent = progress === null ? 0 : progress.bytes > 0 ? progress.received / progress.bytes * 100 : progress.files > 0 ? progress.completed / progress.files * 100 : 0;
  return <Dialog open onOpenChange={(open) => { if (!open) void close(); }} title={title} size="lg" dismissible={!start.isPending && !closing} footer={<>
    {step === 'review' && <div className="me-auto"><Checkbox label={t('importWizard.run.notify')} checked={notify} disabled={uncertain || start.isPending} onCheckedChange={setNotify} /></div>}
    {reason !== null && <span className="me-auto text-sm text-text-muted">{reason}</span>}
    <Button variant={step === 'report' ? 'primary' : 'ghost'} disabled={closing || start.isPending} onClick={() => void close()}>{t(step === 'report' ? 'importWizard.dialog.done' : step === 'run' ? 'common.close' : 'common.cancel')}</Button>
    {(step === 'destination' || step === 'review') && <Button disabled={closing || start.isPending || uncertain} onClick={() => setStep(step === 'review' ? 'destination' : 'source')}>{t('importWizard.dialog.back')}</Button>}
    {step === 'source' && <Button variant="primary" pending={uploading} disabled={!canSource && !uploading} onClick={() => void upload()}>{t(error === null ? 'importWizard.dialog.next' : 'importWizard.dialog.retry')}</Button>}
    {step === 'destination' && <Button variant="primary" disabled={!canDestination} onClick={review}>{t('importWizard.dialog.next')}</Button>}
    {step === 'review' && <Button variant="primary" pending={start.isPending} disabled={!canImport && !start.isPending} onClick={() => start.mutate()}>{t('importWizard.dialog.import')}</Button>}
  </>}>
    <div className="flex min-w-0 flex-col gap-4">
      <ImportSteps at={step} locked={uncertain || start.isPending} onBack={setStep} />
      {step === 'source' && <>
        <ImportSourceStep value={selection} onChange={(next) => void setSourceSelection(next)} onReadingChange={sourceReading} disabled={uploading || closing} />
        {facts?.mode === 'package' && <StateLine compact message={t('importWizard.dialog.package')} />}
        {progress !== null && uploading && <><p role="status" className="text-sm text-text-muted tabular-nums">{t('importWizard.run.uploading', { done: formatNumber(progress.completed, locale), total: formatNumber(progress.files, locale), received: formatBytes(progress.received, locale), bytes: formatBytes(progress.bytes, locale) })}</p><Meter look="progress" value={uploadPercent} label={t('importWizard.steps.source')} /></>}
        {closing && <StateLine compact role="status" message={t('importWizard.dialog.cancelling')} />}
      </>}
      {step === 'destination' && <>
        <ImportDestination value={destination} onChange={changeDestination} standing={standing} isPackage={facts?.mode === 'package'} />
        {facts?.mode === 'package' && <StateLine compact message={t('importWizard.dialog.package')} />}
        {destination.place !== null && (waiting || inspection.isFetching) && <StateLine compact role="status" message={t('importWizard.dialog.checking')} />}
        {inspection.isError && <ErrorAlert error={inspection.error} action={<Button size="sm" onClick={() => void inspection.refetch()}>{t('common.tryAgain')}</Button>} />}
      </>}
      {step === 'review' && <>
        {planBusy && <StateLine compact role="status" message={t('importWizard.dialog.checking')} />}
        {latest === undefined || standing.node === undefined || request === null ? plan.isError ? <ErrorState compact error={plan.error} onRetry={() => void plan.refetch()} /> : <SkeletonRows rows={5} />
          : <fieldset disabled={plan.isPlaceholderData || plan.isError || start.isPending || uncertain} aria-busy={planBusy} className="min-w-0">
            <ImportReview node={standing.node} request={request} plan={latest}
              rows={plan.data?.pages.flatMap((page) => page.objects.items) ?? []} hasMore={plan.hasNextPage} loadingMore={plan.isFetchingNextPage} moreError={plan.isFetchNextPageError ? plan.error : null}
              firstPage={(plan.data?.pageParams[0] === null || plan.data?.pageParams[0] === undefined)} onFirst={() => void cache.resetQueries({ queryKey: importKeys.plan(planned?.node ?? null, planned?.request ?? null, PLAN_VIEW), exact: true })}
              onMore={() => void plan.fetchNextPage()} onOptions={changeOptions} onObject={changeObject} onPerson={changePerson} />
          </fieldset>}
        {plan.isError && latest !== undefined && <ErrorAlert error={plan.error} action={<Button size="sm" onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>} />}
        {start.isError && <ErrorAlert error={start.error} />}
        {uncertain && <StateLine compact tone="warning" message={t('importWizard.dialog.notConfirmed')} />}
      </>}
      {step === 'run' && jobId !== null && <ImportProgress id={jobId} onReport={openReport} />}
      {step === 'report' && reportId !== null && <ImportReport id={reportId} onOpen={() => void close()} />}
      {error !== null && <ErrorAlert error={error} />}
    </div>
  </Dialog>;
}

function ExistingImport({ target, onClose }: { target: Exclude<ImportTarget, { kind: 'new' }>; onClose: () => void }) {
  const { t } = useTranslation();
  const [report, setReport] = useState(target.kind === 'report' ? target.report : null);
  const openReport = useCallback((id: string) => setReport(id), []);
  const step: ImportStep = report === null ? 'run' : 'report';
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t(report === null ? 'importWizard.dialog.title' : 'importWizard.report.title')} size="lg"
    footer={<Button variant="ghost" onClick={onClose}>{t('common.close')}</Button>}>
    <div className="flex min-w-0 flex-col gap-4"><ImportSteps at={step} onBack={() => undefined} />
      {report !== null ? <ImportReport id={report} onOpen={onClose} /> : target.kind === 'job' && <ImportProgress id={target.job} onReport={openReport} />}
    </div>
  </Dialog>;
}

export function ImportDialog({ target, onClose }: { target: ImportTarget; onClose: () => void }) {
  return target.kind === 'new' ? <NewImport target={target} onClose={onClose} /> : <ExistingImport target={target} onClose={onClose} />;
}
