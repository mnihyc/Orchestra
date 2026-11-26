/** Revisioned mount labels, attachment target, whether its data is writable, and what plans preselect for it. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { createMount, mountKey, updateMount } from '@/api/resource-tools';
import type { Mount, MountFate } from '@/generated/api-types';
import { choiceOption } from '@/i18n/choices';
import { EditDialog } from '@/features/resources/EditDialog';
import { LibraryCounts } from '@/features/resources/Library';
import { UpdateNotify } from '@/features/change/UpdateNotify';
import { adminSpec } from '@/ui/AdminAct';
import { HomePicker } from '@/features/resources/HomePicker';
import { useActKey } from '@/lib/forms';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { Textarea } from '@/ui/Textarea';
import { ApplicationPreview, useApplicationImpact } from '@/features/storage/Impact';

/** The three fates in the order plans offer them. */
export const MOUNT_FATES: readonly MountFate[] = ['detach', 'as_is', 'copy'];

export function EditMount({ mount, onClose, onCreated }: { mount?: Mount; onClose: () => void; onCreated?: (mount: Mount) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  const [name, setName] = useState(mount?.name ?? '');
  const [description, setDescription] = useState(mount?.description ?? '');
  const [home, setHome] = useState('');
  const [target, setTarget] = useState(mount?.target ?? '');
  const [readOnly, setReadOnly] = useState(mount?.read_only ?? false);
  const [enabled, setEnabled] = useState(mount?.enabled ?? true);
  const [notify, setNotify] = useState<boolean>();
  const [fate, setFate] = useState<MountFate>(mount?.default_fate ?? 'as_is');
  const enabledChanged = mount !== undefined && enabled !== mount.enabled;
  const changed = mount === undefined ? true : name !== mount.name || description !== mount.description || target !== mount.target || readOnly !== mount.read_only || enabled !== mount.enabled || fate !== mount.default_fate;
  const runtimeChanged=mount!==undefined&&(target!==mount.target||readOnly!==mount.read_only||enabled!==mount.enabled);
  const application=useApplicationImpact({object:{kind:'mount',id:mount?.id??''},model_id:null},runtimeChanged);
  const impact = application.read;
  const save = useMutation({ mutationFn: () => {
    const body = { name: name.trim(), description, home_id: home, target, read_only: readOnly, default_fate: fate };
    return mount === undefined ? createMount(body, actKey(body)) : updateMount(mount.id, { expected_revision: mount.revision, name: name.trim(), description, target, read_only: readOnly, enabled, default_fate: fate, ...(enabledChanged ? { notify: notify ?? impact.data?.notification?.default_on ?? false } : {}) });
  },
    onSuccess: (row) => { if (mount === undefined) cache.setQueryData(mountKey(row.id), row); onCreated?.(row); onClose(); } });
  return <EditDialog admin={adminSpec(mount, 'mount.manage')} title={mount === undefined ? t('resourceDetails.mount.add') : t('resources.edit.title', { name: mount.name })} changed={changed} valid={application.ready && name.trim() !== '' && target.startsWith('/') && (mount !== undefined || home !== '') && (!enabledChanged || !impact.isPending && !impact.isError)} pending={save.isPending} error={save.error} onSave={() => save.mutate()} onClose={onClose}
    footerStart={enabledChanged && <UpdateNotify audience={impact.data?.notification?.audience} checked={notify ?? impact.data?.notification?.default_on ?? false} onChange={setNotify} disabled={save.isPending || impact.isPending || impact.isError} />}>
    <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} data-autofocus />
    <Textarea label={t('resources.listDescription')} value={description} disabled={save.isPending} onChange={(event) => setDescription(event.target.value)} />
    {mount === undefined && <HomePicker value={home} onChange={setHome} disabled={save.isPending} />}
    <Input label={t('resourceDetails.mount.target')} hint={t('resourceDetails.mount.targetHelp')} value={target} disabled={save.isPending} onChange={(event) => setTarget(event.target.value)} />
    <Switch label={t('resourceDetails.mount.readOnly')} checked={readOnly} disabled={save.isPending} onCheckedChange={setReadOnly} />
    {mount !== undefined && <Switch label={t('resourceDetails.enabled')} checked={enabled} disabled={save.isPending} onCheckedChange={setEnabled} />}
    {enabledChanged && impact.isSuccess && <LibraryCounts facts={impact.data.facts} />}
    <Select<MountFate> label={t('resourceDetails.mount.defaultFate')} value={fate} disabled={save.isPending} onValueChange={setFate}
      options={MOUNT_FATES.map((value) => ({ value, ...choiceOption(t, `mountFate.${value}`) }))} />
    {runtimeChanged&&<ApplicationPreview read={application.read} kind={enabled?'change':'disable'}/>}
  </EditDialog>;
}
