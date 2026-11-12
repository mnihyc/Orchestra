/** The same place picker keeps other roots visible while the home writer stays inside its root. */
// Cross-org destinations use the separate atomic move writer and its native deletion footprint.
import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { moveResourceHome } from '@/api/resourceHome';
import { executeMoveOut, previewMoveOut, supportsMoveOut, type MoveOutBody, type MoveOutCopy } from '@/api/moveOut';
import type { ChatDuplicateAudience } from '@/api/duplicates';
import { machinesQuery } from '@/api/resources';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { linkTo } from '@/lib/links';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import type { CopyDestinationJudgement } from '@/api/copyDestinations';
import { CopyPlacePicker } from './CopyPlacePicker';
import { homeMoveReason, type HomeMoveTarget } from './move';

export function MoveHomeDialog({ target, onClose }: { target: HomeMoveTarget; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const navigate = useNavigate();
  const [reader] = useState(libraryReader);
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const key = useActKey();
  const [standing, setStanding] = useState<CopyDestinationJudgement | null>(null);
  const [name, setName] = useState('');
  const [machine, setMachine] = useState('');
  const [notify, setNotify] = useState(false);
  const [review, setReview] = useState(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const across = standing?.cross_root === true;
  const supported = supportsMoveOut(target.kind);
  const custodian = target.kind === 'mcp_server' && target.imported === true;
  const machines = useQuery({ ...machinesQuery, enabled: across && custodian });
  const destination = place === null ? undefined : { kind: 'node' as const, id: place.id };
  const copy: MoveOutCopy = { ...(destination === undefined ? {} : { destination }), ...(review > 0 || target.revision === undefined ? {} : { expected_revision: target.revision }), ...(custodian && machine !== '' ? { machine_id: machine } : {}) };
  const preview = useQuery({ queryKey: ['move-out-plan', reader, target.kind, target.id, target.draft, review, copy], queryFn: ({ signal }) => previewMoveOut(target, copy, signal),
    enabled: across && supported && standing.outcome.state !== 'blocked', staleTime: Infinity, gcTime: 0, retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const plan = preview.data === undefined ? undefined : 'delete_plan' in preview.data ? preview.data.delete_plan : preview.data;
  const audience = preview.data !== undefined && 'notification' in preview.data ? preview.data.notification : undefined;
  const mayNotify = audience !== undefined && audience.people > 0 && audience.people <= audience.notify_limit && audience.preview_id !== null;
  const destroysHistory = plan?.effects.includes('end_group_and_history') === true;
  const reason = place === null ? undefined : across ? standing.outcome.state === 'blocked' ? t(`duplicate.causes.${standing.outcome.cause}`)
    : !supported ? t('duplicate.deletePreviewUnavailable') : preview.isError ? t('duplicate.movePreviewFailed') : plan === undefined || preview.isFetching ? t('duplicate.checking')
      : destroysHistory && name !== target.name ? t('duplicate.typeName') : notify && !mayNotify ? t('duplicate.counting') : false : homeMoveReason(target, place, t);
  const intent = JSON.stringify({ destination: place?.id, machine, notify });
  const write = useMutation({ mutationFn: async (asked: { intent: string; destination: string; body?: MoveOutBody; key: string; audience?: ChatDuplicateAudience }) => {
    if (!sameLibraryReader(reader)) throw new DOMException('The requested home is no longer available.', 'AbortError');
    if (asked.body !== undefined) return executeMoveOut(target, asked.body, asked.key);
    if (place === null || homeMoveReason(target, place, t) !== false) throw new DOMException('The requested home is no longer available.', 'AbortError');
    await moveResourceHome(target.kind, target.id, { home_id: place.id, ...(target.revision === undefined ? {} : { expected_revision: target.revision }) }, asked.key);
    return null;
  }, onSuccess: (receipt) => {
    if (!mounted.current || !sameLibraryReader(reader)) return;
    toast({ tone: 'success', title: t(receipt === null ? 'duplicate.homeSaved' : 'duplicate.movedAcross') }); onClose();
    if (receipt !== null) void navigate({ href: linkTo(receipt) });
  } });
  const unchanged = write.variables?.intent === intent;
  const retry = write.isError && unchanged;
  const shownAudience = unchanged && (write.isError || write.isPending) ? write.variables.audience : audience;
  const destructive = unchanged && (write.isError || write.isPending) ? write.variables.body !== undefined : across;
  const submit = () => {
    if (write.isPending || place === null || !sameLibraryReader(reader)) return;
    if (retry) { write.mutate(write.variables); return; }
    if (reason !== false) return;
    const body: MoveOutBody | undefined = across && plan !== undefined ? { copy: { ...copy, expected_revision: plan.revision }, delete_plan: { revision: plan.revision, stamp: plan.stamp },
      ...(target.kind === 'chat' ? { notify: notify ? true : null, notify_preview_id: notify ? audience?.preview_id ?? null : null } : {}) } : undefined;
    write.mutate({ intent, destination: place.id, ...(body === undefined ? {} : { body: structuredClone(body) }), key: key(body ?? { home_id: place.id, revision: target.revision }), ...(audience === undefined ? {} : { audience: structuredClone(audience) }) });
  };
  const reviewAgain = () => { write.reset(); key.reset(); setNotify(false); setName(''); setReview((before) => before + 1); };
  return <Dialog open dismissible={!write.isPending} title={t('duplicate.moveTitle', { name: target.name })} onOpenChange={(open) => { if (!open && !write.isPending) onClose(); }}
    footer={<>{destructive && target.kind === 'chat' && <div className="me-auto min-w-0">{shownAudience === undefined ? <Checkbox checked={false} onCheckedChange={setNotify} disabled label={t('duplicate.counting')} />
      : <NotifyPeople plan={{ preview_id: shownAudience.preview_id, people: shownAudience.people, default_on: false }} value={notify} onChange={(value) => { setNotify(value); write.reset(); }} disabled={write.isPending || !mayNotify && !notify} />}</div>}
      <span className="me-auto min-w-0 text-sm text-text-muted">{reason}</span><Button variant="ghost" disabled={write.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant={destructive ? 'danger' : 'primary'} pending={write.isPending} disabled={place === null || !retry && reason !== false} onClick={submit}>{destructive ? t('duplicate.moveAcross', { org: place?.rootName ?? t('access.place.hidden') }) : t('duplicate.moveConfirm')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-4"><CopyPlacePicker label={t('duplicate.destination')} subject={{ kind: target.kind, id: target.id }} sourceName={target.name} intent="move" onStanding={setStanding} homeMoveRoot={target.rootOrgId} homeMoveRefusal={(candidate) => homeMoveReason(target, candidate, t)} value={place} disabled={write.isPending} onChange={(next) => { setPlace(next); setName(''); setNotify(false); write.reset(); }} />
      {across && custodian && <><Select label={t('duplicate.machine')} value={machine} onValueChange={(value) => { setMachine(value); setNotify(false); write.reset(); }} disabled={write.isPending}
        options={[{ value: '', name: t('duplicate.sameMachine') }, ...(machines.data ?? []).map((row) => ({ value: row.id, name: row.name }))]} />
        {machines.isError && <ErrorState compact error={machines.error} onRetry={() => void machines.refetch()} />}</>}
      {across && supported && (preview.isError ? <ErrorState compact error={preview.error} onRetry={reviewAgain} /> : plan === undefined ? <SkeletonRows read={preview} rows={2} /> : <Table label={t('duplicate.deletePlan')} rows={plan.effects} rowKey={(effect) => effect} columns={[{ id: 'effect', header: t('duplicate.effect'), cell: (effect) => t(`duplicate.moveEffects.${effect}`) }]} />)}
      {across && destroysHistory && <Input label={t('duplicate.typeName')} value={name} onChange={(event) => setName(event.target.value)} disabled={write.isPending} autoComplete="off" />}
      {across && audience !== undefined && audience.people > audience.notify_limit && <span className="text-sm text-text-muted">{t('duplicate.notifyLimit', { number: formatNumber(audience.notify_limit, locale) })}</span>}
      {write.isError && <ErrorAlert error={write.error} action={write.error instanceof ApiError && write.error.is('conflict.stale_revision') ? <Button onClick={reviewAgain}>{t('duplicate.reviewAgain')}</Button> : undefined} />}
    </div>
  </Dialog>;
}
