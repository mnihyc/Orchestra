/** The same compact pool editor starts from an authored live group rule or an existing pool. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createPool, readPoolAfterWrite, updatePool } from '@/api/pools';
import type { Pool, PoolCreateBody, PoolKind, PoolMemberRule, PoolPolicy, PoolLoadMeasure, PoolDisplayGroup, PoolReceipt, PoolUpdateBody } from '@/generated/api-types';
import { EditDialog } from '@/features/resources/EditDialog';
import { HomePicker } from '@/features/resources/HomePicker';
import { useActKey } from '@/lib/forms';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Checkbox } from '@/ui/Checkbox';

export interface PoolDraft {
  kind: PoolKind;
  rule?: PoolMemberRule;
  group?: PoolDisplayGroup;
  kinds?: readonly PoolKind[];
  home?: { id: string; name: string };
  name?: string;
}
const kinds: readonly PoolKind[] = ['model', 'provider', 'environment', 'host'];
const policies = ['ordered', 'weighted', 'round_robin', 'least_cost', 'least_loaded'] as const;

export function PoolDialog({ initial, pool: offered, onClose, onCreated }: {
  initial?: PoolDraft; pool?: Pool; onClose: () => void; onCreated?: (id: string) => void;
}) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const key = useActKey();
  const confirmed=useRef<PoolReceipt | null>(null);
  const [accepted,setAccepted]=useState(false);
  const [pool] = useState(offered);
  const [name, setName] = useState(pool?.name ?? initial?.name ?? '');
  const [home, setHome] = useState(initial?.home?.id ?? '');
  const [kind, setKind] = useState<PoolKind>(pool?.kind ?? initial?.kind ?? 'model');
  const [policy, setPolicy] = useState<PoolPolicy['type'] | ''>(pool?.policy?.type ?? '');
  const [measure, setMeasure] = useState<PoolLoadMeasure>(pool?.policy?.type === 'least_loaded' ? pool.policy.measure : 'combined');
  const [enabled, setEnabled] = useState(pool?.enabled ?? false);
  const [lend, setLend] = useState(pool?.lend_members ?? false);
  const [memberId] = useState(() => crypto.randomUUID());
  const rule: PoolMemberRule | undefined = initial?.group === undefined ? initial?.rule : { type: 'grouping', kind, group: initial.group };
  const member = rule === undefined ? [] : [{ id: memberId, rule, enabled: true, priority: null, weight: null }];
  const selected: PoolPolicy | null = policy === '' ? null : policy === 'least_loaded' ? { type: policy, measure } : { type: policy };
  const allowed = policies.filter((value) => value === 'least_cost' ? kind === 'model' || kind === 'provider'
    : value === 'least_loaded' ? kind === 'host' || kind === 'environment' : true);
  const valid = name.trim() !== '' && (pool !== undefined || home !== '') && (!enabled || selected !== null) && (policy === '' || allowed.includes(policy));
  const save = useMutation({ mutationFn: async () => {
    if (confirmed.current !== null) { await readPoolAfterWrite(cache,confirmed.current.id);return confirmed.current; }
    if (!valid) throw new Error(t('pools.form.invalid'));
    if (pool !== undefined) {
      const body: PoolUpdateBody = { expected_revision: pool.revision, name: name.trim(), description: pool.description,
        enabled, policy: selected, lend_members: lend, config: null };
      confirmed.current=await updatePool(pool.id, body);setAccepted(true);
      await readPoolAfterWrite(cache,confirmed.current.id);return confirmed.current;
    }
    const body: PoolCreateBody = { home_id: home, name: name.trim(), description: '', access: 'only_me',
      definition: { kind, enabled, policy: selected, lend_members: lend, config: {}, members: member } };
    confirmed.current=await createPool(body, key(body));setAccepted(true);
    await readPoolAfterWrite(cache,confirmed.current.id);return confirmed.current;
  }, onSuccess: (receipt) => {

    onCreated?.(receipt.id); onClose();
  } });
  return <EditDialog title={t(pool === undefined ? 'pools.new' : 'pools.edit')} submitLabel={t(!accepted ? pool === undefined ? 'pools.create' : 'common.save' : 'pools.readAgain')}
    changed valid={valid} pending={save.isPending} error={save.error} onSave={() => save.mutate()} onClose={onClose}>
    <Input label={t('pools.name')} value={name} onChange={(event) => setName(event.target.value)} disabled={save.isPending || accepted} />
    {pool === undefined && <HomePicker compact value={home} onChange={setHome} disabled={save.isPending || accepted}
      places={initial?.home === undefined ? [] : [initial.home]} />}
    <Select label={t('pools.kind')} value={kind} onValueChange={setKind} disabled={pool !== undefined || initial?.rule !== undefined || save.isPending || accepted}
      options={(initial?.kinds ?? kinds).map((value) => ({ value, name: t(`pools.kinds.${value}`) }))} />
    <Select label={t('pools.policy.label')} value={policy} onValueChange={setPolicy} placeholder={t('pools.policy.choose')} disabled={save.isPending || accepted}
      options={allowed.map((value) => ({ value, name: t(`pools.policy.${value}`) }))} />
    {policy === 'least_loaded' && <Select label={t('pools.measure.label')} value={measure} onValueChange={setMeasure} disabled={save.isPending || accepted}
      options={(['cpu', 'memory', 'combined', 'threads', 'processes'] as const).map((value) => ({ value, name: t(`pools.measure.${value}`) }))} />}
    <Checkbox label={t('pools.enabled')} checked={enabled} onCheckedChange={setEnabled} disabled={save.isPending || accepted} />
    <Checkbox label={t('pools.lend')} checked={lend} onCheckedChange={setLend} disabled={save.isPending || accepted} />
  </EditDialog>;
}
