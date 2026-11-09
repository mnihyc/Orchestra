/** The ordinary topic creator can start a declared no-environment program. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { askModelsQuery, askReader } from '@/api/ask';
import { nodeQuery } from '@/api/nodes';
import { createThread } from '@/api/threads';
import { AskModelChoice } from '@/features/ask/ModelChoice';
import type { PlaceRef } from '@/features/navigation/places';
import type { AskModel, CreateThread } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import type { StartDraft, ThreadPreparation } from './BindingDialog';

export function ProgramThread({ parent, open, draft, onClose, onNative, prepare }: {
  parent: PlaceRef; open: boolean; draft: StartDraft; onClose: () => void; onNative: (draft: StartDraft) => void; prepare?: ThreadPreparation;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const place = useQuery({ ...nodeQuery(parent.id), enabled: open });
  const root = place.data?.org_id ?? '';
  const models = useQuery({ ...askModelsQuery(root), enabled: open && root !== '' });
  const [name, setName] = useState(draft.name ?? prepare?.initial?.name ?? '');
  const [message, setMessage] = useState(draft.message);
  const [model, setModel] = useState<AskModel | null>(draft.program === undefined ? prepare?.initial?.program?.model ?? null : draft.program.model ?? null);
  const [access, setAccess] = useState<'private' | 'defaults'>(draft.generalAccess === 'defaults' || draft.generalAccess === undefined && prepare !== undefined && prepare.initial?.access !== 'private' ? 'defaults' : 'private');
  const [first] = useState(newActKey);
  const key = useActKey();
  const listing = models.data;
  const selected = model ?? listing?.default_model ?? null;
  const allowed = selected !== null && listing !== undefined && listing.blocked_reason === null && listing.models.some((option) => option.allowed && option.selection.provider_id === selected.provider_id && option.selection.model === selected.model);
  const requiresModel = message.trim() !== '' || model !== null;
  const question = message.trim();
  const body: CreateThread = {
    program: { id: 'orch', ...(model === null ? {} : { model }) },
    ...(name.trim() === '' ? {} : { name: name.trim() }),
    ...(access === 'private' ? { access: 'private' } : {}),
    ...(question === '' ? {} : { first_input: { client_id: first, content: [{ type: 'text', text: question }] } }),
  };
  const create = useMutation({
    mutationFn: async () => {
      const current = askReader();
      const thread = await createThread(parent.id, body, key({ topic: parent.id, body }));
      current();
      onClose();
      await navigate({ to: '/thread/$threadId', params: { threadId: thread.id } });
    },
  });
  const choose = () => {
    if (prepare === undefined) { create.mutate(); return; }
    const { first_input: _input, ...config } = body;
    prepare.onChoose(config); onClose();
  };
  const ready = parent.kind === 'topic' && place.data?.kind === 'topic' && root !== '' && (!requiresModel || allowed);
  return <Dialog open={open} onOpenChange={(next) => { if (!next && !create.isPending) onClose(); }}
    title={t('start.title', { parent: parent.name })} dismissible={!create.isPending}
    footer={<><Button disabled={create.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" pending={create.isPending} disabled={!ready} onClick={choose}>{t(prepare === undefined ? 'start.create' : 'sendLater.useSetup')}</Button></>}>
    <div className="flex min-w-0 flex-col gap-4 py-2" data-program-thread>
      <div><h3 className="font-medium">{t('ask.threadProgram')}</h3><p className="text-sm text-text-muted">{t('ask.threadProgramDescription')}</p><p className="text-sm text-text-muted">{t('ask.noEnvironment')}</p></div>
      <Button size="sm" variant="ghost" className="self-start" disabled={create.isPending} onClick={() => onNative({ ...draft, name, message, program: { id: 'orch', model }, generalAccess: access })}>{t('ask.chooseNativeProgram')}</Button>
      {place.isError && <ErrorState compact error={place.error} onRetry={() => void place.refetch()} />}
      <Input label={t('start.options.name')} value={name} disabled={create.isPending} onChange={(event) => setName(event.target.value)} />
      <AskModelChoice root={root} value={model} disabled={create.isPending || root === ''} onChange={setModel} />
      {models.isError && <ErrorState compact error={models.error} onRetry={() => void models.refetch()} />}
      {models.isSuccess && selected === null && <p className="text-sm text-text-muted">{t('ask.noModels')}</p>}
      {requiresModel && selected !== null && models.isSuccess && !allowed && <p className="text-sm text-text-muted">{t('ask.modelUnavailable')}</p>}
      <Select label={t('ask.programAudience')} value={access} disabled={create.isPending} onValueChange={(value) => setAccess(value === 'private' ? 'private' : 'defaults')}
        options={[{ value: 'private', label: t('ask.programPrivate') }, { value: 'defaults', label: t('ask.programDefaults') }]} />
      {prepare === undefined && <Textarea label={t('start.message.title')} value={message} disabled={create.isPending} onChange={(event) => setMessage(event.target.value)} />}
      {create.error !== null && <ErrorAlert error={create.error} />}
    </div>
  </Dialog>;
}
