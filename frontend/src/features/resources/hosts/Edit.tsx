/** Quick Rename keeps its opening revision; D443 routes all other host edits to the one Settings surface. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';

import { hostKey, updateHost } from '@/api/resources';
import { ApiError } from '@/api/client';
import { EditDialog } from '@/features/resources/EditDialog';
import type { Host } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

export interface EditHostProps {
  host: Host;
  onClose: () => void;
}

export function EditHost({ host, onClose }: EditHostProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [name, setName] = useState(host.name);
  const nameChanged = name.trim() !== host.name;
  const save = useMutation({
    mutationFn: () =>
      updateHost(host.id, {
        expected_revision: host.revision,
        ...(nameChanged ? { name: name.trim() } : {}),
      }),
    onSuccess: (updated) => {
      queryClient.setQueryData<Host>(hostKey(updated.id), (latest) => latest !== undefined && latest.revision > updated.revision ? latest : updated);
      toast({ tone: 'success', title: t('resources.edit.saved', { name: updated.name }) });
      onClose();
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: (error) => { if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: hostKey(host.id) }); },
  });

  return (
    <EditDialog
      admin={adminSpec(host, 'host.manage')}
      title={t('resources.edit.renameTitle', { name: host.name })}
      changed={nameChanged}
      valid={name.trim() !== ''}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />
    </EditDialog>
  );
}
