/** One host keeps native copy acknowledgements independent of readable resource projections. */
import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { duplicate, duplicateDocument, previewChatDuplicate, type ChatDuplicateAudience, type DuplicateBody, type DuplicateOptions } from '@/api/duplicates';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { machinesQuery } from '@/api/resources';
import { type PickedPlace } from '@/features/settings/PlacePicker';
import type { CopyDestinationJudgement } from '@/api/copyDestinations';
import { CopyPlacePicker } from './CopyPlacePicker';
import type { DuplicateTarget } from '@/features/duplicate/target';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { linkTo } from '@/lib/links';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { CopySetupBefore, rememberCopySetup } from './CopySetup';

interface CopyAttempt { body: DuplicateBody; options: DuplicateOptions; key: string; intent: string; review?: ChatDuplicateAudience }

export function DuplicateDialog({ target, quick = false, onClose }: { target: DuplicateTarget; quick?: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const navigate = useNavigate();
  const cache = useQueryClient();
  const key = useActKey();
  const [reader] = useState(libraryReader);
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const [name, setName] = useState('');
  const [machine, setMachine] = useState('');
  const [placement, setPlacement] = useState('same');
  const [standing, setStanding] = useState<CopyDestinationJudgement | null>(null);
  const [notify, setNotify] = useState(false);
  const [quickIntent] = useState<CopyAttempt>(() => ({
    body: target.revision === undefined ? {} : { expected_revision: target.revision },
    options: target.draft === true ? { draft: true } : {}, key: newActKey(), intent: 'quick',
  }));
  const automatic = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const audience = useQuery({ queryKey: ['chat-duplicate-audience', target.id, reader], queryFn: ({ signal }) => previewChatDuplicate(target.id, signal), enabled: target.kind === 'chat' && !quick, retry: false, staleTime: Infinity, refetchOnWindowFocus: false, refetchOnReconnect: false, gcTime: 0 });
  const machines = useQuery({ ...machinesQuery, enabled: !quick && (target.kind === 'mcp_server' && target.imported === true || target.kind === 'environment' && placement === 'machine') });
  const mayNotify = audience.isSuccess && audience.data.people > 0 && audience.data.people <= audience.data.notify_limit && audience.data.preview_id !== null;
  const body: DuplicateBody = {
    ...(name.trim() === '' ? {} : { name: name.trim() }),
    ...(place === null ? {} : { destination: { kind: 'node', id: place.id } }),
    ...(target.revision === undefined ? {} : { expected_revision: target.revision }),
  };
  const options: DuplicateOptions = {
    ...(target.draft === true ? { draft: true } : {}),
    ...(machine === '' || target.kind === 'environment' && placement !== 'machine' ? {} : { machineId: machine }),
    ...(target.kind === 'environment' && placement === 'managed' ? { managed: {} } : {}),
    ...(notify && mayNotify && audience.data.preview_id !== null ? { notify: { preview: audience.data.preview_id } } : {}),
  };
  const write = useMutation({ mutationFn: async (asked: CopyAttempt) => {
    if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed.', 'AbortError');
    if (target.kind === 'document') {
      const at = asked.body.destination?.kind === 'node' ? { kind: 'node_docs' as const, id: asked.body.destination.id } : target.directory;
      if (at === undefined) throw new Error('The document copy has no current destination.');
      return duplicateDocument(target.id, at, asked.body.name ?? undefined, asked.key);
    }
    if (target.kind === 'thread') throw new Error('Threads use the native fork planner.');
    return duplicate(target.kind, target.id, asked.body, asked.key, asked.options);
  }, onSuccess: (receipt) => {
    if (!mounted.current || !sameLibraryReader(reader)) return;
    if ('projection' in receipt) rememberCopySetup(cache, receipt);
    onClose();
    // A receipt identifies the new thing; ordinary opening decides whether it can be read.
    // Copy links use the public address; internal navigation preserves this reader's receipt.
    const destination = new URL(linkTo({ kind: receipt.kind, id: receipt.id }));
    void navigate({ href: `${destination.pathname}${destination.search}${destination.hash}` });
  } });
  const { mutate } = write;
  const intent = JSON.stringify({ name: name.trim(), destination: place?.id, machine, placement, notify });
  const unchanged = write.variables !== undefined && (quick || write.variables.intent === intent);
  const retry = write.isError && unchanged;
  const review = unchanged && (write.isError || write.isPending) ? write.variables.review : audience.data;
  const choiceReady = standing?.outcome.state === 'allowed' || standing?.outcome.state === 'needs_choice' && (
    standing.outcome.choice === 'native_operation_scope' || standing.outcome.choice === 'environment_placement' && target.kind === 'environment' && (placement !== 'machine' || machine !== '') || standing.outcome.choice === 'mcp_machine' && target.kind === 'mcp_server' && machine !== '');
  const send = () => {
    if (write.isPending || !sameLibraryReader(reader) || target.refusal !== undefined) return;
    if (retry) { mutate(write.variables); return; }
    if (!quick && !choiceReady || notify && (!mayNotify || audience.isFetching)) return;
    mutate({ body, options, key: quick ? quickIntent.key : key({ body, options }), intent, ...(audience.data === undefined ? {} : { review: audience.data }) });
  };
  useEffect(() => {
    if (quick && !automatic.current && target.refusal === undefined) { automatic.current = true; mutate(quickIntent); }
  }, [quick, target.refusal, mutate, quickIntent]);
  const pending = write.isPending || quick && write.isIdle;
  const choosePlace = (next: PickedPlace | null) => { setPlace(next); write.reset(); };
  return <Dialog open title={t('duplicate.title', { name: target.name })} onOpenChange={(open) => { if (!open && !pending) onClose(); }}
    footer={<>
      {!quick && target.kind === 'chat' && <div className="me-auto min-w-0">{review === undefined ? <Checkbox checked={false} onCheckedChange={setNotify} disabled label={t('duplicate.counting')} />
        : <NotifyPeople plan={{ preview_id: review.preview_id, people: review.people, default_on: false }} value={notify} onChange={setNotify} disabled={pending || !mayNotify && !notify} />}</div>}
      <Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={pending} disabled={target.refusal !== undefined || !quick && !retry && (!choiceReady || place === null) || !retry && notify && (!mayNotify || audience.isFetching)} onClick={send}>{t(pending ? 'duplicate.making' : write.isError ? 'common.tryAgain' : 'duplicate.now')}</Button>
    </>}>
    <div className="flex min-w-0 flex-col gap-4">
      <CopySetupBefore kind={target.kind} notes={target.refusal === undefined ? target.setup : undefined} />
      {!quick && <>
        <CopyPlacePicker label={t('duplicate.destination')} subject={{ kind: target.kind, id: target.id }} sourceName={target.name} intent="duplicate" onStanding={setStanding} value={place} onChange={choosePlace} disabled={pending} includeThreads={target.kind === 'document'} />
        <Input label={t('duplicate.name')} optional value={name} onChange={(event) => { setName(event.target.value); write.reset(); }} disabled={pending} />
        {target.kind === 'environment' && <Select label={t('duplicate.placement')} value={placement} onValueChange={(value) => { setPlacement(value); write.reset(); }} disabled={pending}
          options={[{ value: 'same', name: t('duplicate.samePlacement') }, { value: 'managed', name: t('duplicate.managedPlacement') }, { value: 'machine', name: t('duplicate.machinePlacement') }]} />}
        {(target.kind === 'mcp_server' && target.imported === true || target.kind === 'environment' && placement === 'machine') && <>
          <Select label={t('duplicate.machine')} value={machine} onValueChange={(value) => { setMachine(value); write.reset(); }} disabled={pending}
            options={[{ value: '', name: t('duplicate.sameMachine') }, ...(machines.data ?? []).map((row) => ({ value: row.id, name: row.name }))]} />
          {machines.isError && <ErrorAlert error={machines.error} action={<Button onClick={() => void machines.refetch()}>{t('common.tryAgain')}</Button>} />}
        </>}
        {target.kind === 'chat' && audience.isError && <ErrorAlert error={audience.error} action={<Button onClick={() => void audience.refetch()}>{t('common.tryAgain')}</Button>} />}
        {target.kind === 'chat' && audience.data !== undefined && audience.data.people > audience.data.notify_limit && <p className="text-sm text-text-muted">{t('duplicate.notifyLimit', { number: formatNumber(audience.data.notify_limit, locale) })}</p>}
      </>}
      {quick && pending && <SkeletonRows rows={1} />}
      {target.refusal !== undefined && <p className="text-sm text-text-muted">{target.refusal}</p>}
      {write.isError && <ErrorAlert error={write.error} action={target.kind === 'chat' && write.error instanceof ApiError && write.error.is('conflict.stale_revision') ? <Button onClick={() => { write.reset(); key.reset(); setNotify(false); void audience.refetch(); }}>{t('duplicate.reviewAgain')}</Button> : undefined} />}
    </div>
  </Dialog>;
}
