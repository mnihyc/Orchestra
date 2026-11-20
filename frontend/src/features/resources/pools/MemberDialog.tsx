/** One member has its own revision-checked edit; defaults stay inherited until explicitly set. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { putPoolMember, putPoolMemberMetadata, readPoolAfterWrite } from '@/api/pools';
import type { Pool, PoolMemberRule, PoolMemberView, BackendRef } from '@/generated/api-types';
import { adminSpec } from '@/ui/AdminAct';
import { EditDialog } from '@/features/resources/EditDialog';
import { ExactBackendChoice } from '@/features/settings/CapabilityBackendChoice';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { Input } from '@/ui/Input';
import { Checkbox } from '@/ui/Checkbox';
import { Select } from '@/ui/Select';
import { PoolResourceChoice } from './ResourceChoice';

export function MemberDialog({ pool, member, onClose, onSaved }: { pool: Pool; member?: PoolMemberView; onClose: () => void; onSaved?: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [id] = useState(member?.id ?? crypto.randomUUID());
  const [enabled, setEnabled] = useState(member?.enabled ?? true);
  const [priority, setPriority] = useState(member?.priority?.toString() ?? '');
  const [weight, setWeight] = useState(member?.weight ?? '');
  const [position, setPosition] = useState(member?.position.toString() ?? '');
  const [mode, setMode] = useState('single');
  const [backend,setBackend]=useState<BackendRef | null>(null);
  const [resource, setResource] = useState('');
  const [template,setTemplate]=useState('');
  const [group, setGroup] = useState('');
  const family = mode === 'pool' ? 'pools' : mode === 'host_environments' || mode === 'template_on_host' ? 'hosts' : pool.kind === 'model' || pool.kind === 'provider' ? 'providers' : pool.kind === 'host' ? 'hosts' : 'environments';
  const rule: PoolMemberRule | null = member?.rule ?? (mode === 'exact' ? backend === null ? null : {type:'single',resource:{kind:'model',backend}} : resource === '' ? null : mode === 'pool' ? { type: 'pool', id: resource, kind: pool.kind }
    : mode === 'host_environments' ? {type:'host_environments',host_id:resource}
    : mode === 'template_on_host' ? template === '' ? null : {type:'template_on_host',host_id:resource,template_id:template}
    : pool.kind === 'model' ? { type: 'provider_models', provider_id: resource, group: group.trim() === '' ? null : group.trim() }
      : { type: 'single', resource: { kind: pool.kind, id: resource } });
  const valid = (member !== undefined || rule !== null) && (position === '' && member === undefined || /^-?\d+$/.test(position) && Number.isSafeInteger(Number(position))) && (priority === '' || /^\d+$/.test(priority) && Number.isSafeInteger(Number(priority)))
    && (weight === '' || /^(?:\d+(?:\.\d*)?|\.\d+)$/.test(weight) && /[1-9]/.test(weight));
  const save = useMutation({ mutationFn: () => {
    if (member !== undefined) return putPoolMemberMetadata(pool.id,member.id,{expected_revision:pool.revision,enabled,priority:priority === '' ? null:Number(priority),weight:weight === '' ? null:weight,position:Number(position)});
    if (!valid || rule === null) throw new Error(t('pools.form.invalid'));
    return putPoolMember(pool.id, { expected_revision: pool.revision, position: position === '' ? null:Number(position), member: { id, rule, enabled,
      priority: priority === '' ? null : Number(priority), weight: weight === '' ? null : weight } });
  }, onSuccess: async () => { await readPoolAfterWrite(cache,pool.id);onSaved?.(); onClose(); } });
  return <EditDialog admin={adminSpec(pool, 'pool.manage')} title={t(member === undefined ? 'pools.addMember' : 'pools.editMember')} changed valid={valid} pending={save.isPending} error={save.error} onClose={onClose} onSave={() => save.mutate()}>
    {member === undefined && <><Select label={t('pools.member')} value={mode} disabled={save.isPending} onValueChange={(value) => { setMode(value); setResource(''); }}
      options={[{ value: 'single', name: t(pool.kind === 'model' ? 'pools.rules.provider_models' : 'pools.rules.single') }, { value: 'pool', name: t('pools.rules.pool') }, ...(pool.kind === 'model' ? [{ value: 'exact', name: t('pools.exactModel') }] : []), ...(pool.kind === 'environment' ? [{value:'host_environments',name:t('pools.rules.host_environments')},{value:'template_on_host',name:t('pools.rules.template_on_host')}] : [])]} />
      {mode === 'exact' ? <ExactBackendChoice place={pool.home?.id ?? ''} value={backend} onChange={setBackend} label={t('pools.member')} /> : <PoolResourceChoice key={family} disabled={save.isPending} kind={family} poolKind={mode === 'pool' ? pool.kind : undefined} value={resource} onChange={setResource} label={t('pools.member')} />}
      {mode === 'template_on_host' && <PoolResourceChoice disabled={save.isPending} kind="templates" value={template} onChange={setTemplate} label={t('pools.template')} />}
      {pool.kind === 'model' && mode === 'single' && <Input disabled={save.isPending} label={t('pools.modelGroup')} value={group} onChange={(event) => setGroup(event.target.value)} />}</>}
    <Checkbox label={t('pools.enabled')} checked={enabled} onCheckedChange={setEnabled} disabled={save.isPending} />
    <ResourceDetails><div className="flex flex-col gap-3"><Input disabled={save.isPending} label={t('pools.position')} placeholder={t('pools.append')} inputMode="numeric" value={position} onChange={(event) => setPosition(event.target.value)} />
      <Input disabled={save.isPending} label={t('pools.priority')} placeholder={t('pools.inherited')} inputMode="numeric" value={priority} onChange={(event) => setPriority(event.target.value)} />
      <Input disabled={save.isPending} label={t('pools.weight')} placeholder={t('pools.inherited')} inputMode="decimal" value={weight} onChange={(event) => setWeight(event.target.value)} /></div></ResourceDetails>
  </EditDialog>;
}
