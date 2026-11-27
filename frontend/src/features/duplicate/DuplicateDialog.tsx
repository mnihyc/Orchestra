/** One host keeps native copy acknowledgements independent of readable resource projections. */
import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { duplicate, duplicateDocument, type DuplicateBody, type DuplicateOptions } from '@/api/duplicates';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { machinesQuery } from '@/api/resources';
import { type PickedPlace } from '@/features/settings/PlacePicker';
import type { CopyDestinationJudgement } from '@/api/copyDestinations';
import { CopyPlacePicker } from './CopyPlacePicker';
import type { DuplicateTarget } from '@/features/duplicate/target';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { linkTo } from '@/lib/links';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { CopySetupBefore, rememberCopySetup } from './CopySetup';

interface CopyAttempt { body: DuplicateBody; options: DuplicateOptions; key: string; intent: string }

export function DuplicateDialog({ target, quick = false, onClose }: { target: DuplicateTarget; quick?: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cache = useQueryClient();
  const key = useActKey();
  const [reader] = useState(libraryReader);
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const [name, setName] = useState('');
  const [machine, setMachine] = useState('');
  const [placement, setPlacement] = useState('same');
  const [standing, setStanding] = useState<CopyDestinationJudgement | null>(null);
  const [quickIntent] = useState<CopyAttempt>(() => ({
    body: target.revision === undefined ? {} : { expected_revision: target.revision },
    options: target.draft === true ? { draft: true } : {}, key: newActKey(), intent: 'quick',
  }));
  const automatic = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const machines = useQuery({ ...machinesQuery, enabled: !quick && (target.kind === 'mcp_server' && target.imported === true || target.kind === 'environment' && placement === 'machine') });
  const body: DuplicateBody = {
    ...(name.trim() === '' ? {} : { name: name.trim() }),
    ...(place === null ? {} : { destination: { kind: 'node', id: place.id } }),
    ...(target.revision === undefined ? {} : { expected_revision: target.revision }),
  };
  const options: DuplicateOptions = {
    ...(target.draft === true ? { draft: true } : {}),
    ...(machine === '' || target.kind === 'environment' && placement !== 'machine' ? {} : { machineId: machine }),
    ...(target.kind === 'environment' && placement === 'managed' ? { managed: {} } : {}),
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
  const intent = JSON.stringify({ name: name.trim(), destination: place?.id, machine, placement });
  const unchanged = write.variables !== undefined && (quick || write.variables.intent === intent);
  const retry = write.isError && unchanged;
  const choiceReady = standing?.outcome.state === 'allowed' || standing?.outcome.state === 'needs_choice' && (
    standing.outcome.choice === 'native_operation_scope' || standing.outcome.choice === 'environment_placement' && target.kind === 'environment' && (placement !== 'machine' || machine !== '') || standing.outcome.choice === 'mcp_machine' && target.kind === 'mcp_server' && machine !== '');
  const send = () => {
    if (write.isPending || !sameLibraryReader(reader) || target.refusal !== undefined) return;
    if (retry) { mutate(write.variables); return; }
    if (!quick && !choiceReady) return;
    mutate({ body, options, key: quick ? quickIntent.key : key({ body, options }), intent });
  };
  useEffect(() => {
    if (quick && !automatic.current && target.refusal === undefined) { automatic.current = true; mutate(quickIntent); }
  }, [quick, target.refusal, mutate, quickIntent]);
  const pending = write.isPending || quick && write.isIdle;
  const choosePlace = (next: PickedPlace | null) => { setPlace(next); write.reset(); };
  return <Dialog open title={t('duplicate.title', { name: target.name })} onOpenChange={(open) => { if (!open && !pending) onClose(); }}
    footer={<>
      <Button variant="ghost" disabled={pending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={pending} disabled={target.refusal !== undefined || !quick && !retry && (!choiceReady || place === null)} onClick={send}>{t(pending ? 'duplicate.making' : write.isError ? 'common.tryAgain' : 'duplicate.now')}</Button>
    </>}>
    <div className="flex min-w-0 flex-col gap-4">
      <CopySetupBefore kind={target.kind} notes={target.refusal === undefined ? target.setup : undefined} />
      {!quick && <>
        <Input label={t('duplicate.name')} optional value={name} onChange={(event) => { setName(event.target.value); write.reset(); }} disabled={pending} />
        <CopyPlacePicker label={t('duplicate.destination')} subject={{ kind: target.kind, id: target.id }} sourceName={target.name} intent="duplicate" onStanding={setStanding} value={place} onChange={choosePlace} disabled={pending} includeThreads={target.kind === 'document'} />
        {target.kind === 'environment' && <Select label={t('duplicate.placement')} value={placement} onValueChange={(value) => { setPlacement(value); write.reset(); }} disabled={pending}
          options={[{ value: 'same', name: t('duplicate.samePlacement') }, { value: 'managed', name: t('duplicate.managedPlacement') }, { value: 'machine', name: t('duplicate.machinePlacement') }]} />}
        {(target.kind === 'mcp_server' && target.imported === true || target.kind === 'environment' && placement === 'machine') && <>
          <Select label={t('duplicate.machine')} value={machine} onValueChange={(value) => { setMachine(value); write.reset(); }} disabled={pending}
            options={[{ value: '', name: t('duplicate.sameMachine') }, ...(machines.data ?? []).map((row) => ({ value: row.id, name: row.name }))]} />
          {machines.isError && <ErrorAlert error={machines.error} action={<Button onClick={() => void machines.refetch()}>{t('common.tryAgain')}</Button>} />}
        </>}
      </>}
      {quick && pending && <SkeletonRows rows={1} />}
      {target.refusal !== undefined && <p className="text-sm text-text-muted">{target.refusal}</p>}
      {write.isError && <ErrorAlert error={write.error} />}
    </div>
  </Dialog>;
}
