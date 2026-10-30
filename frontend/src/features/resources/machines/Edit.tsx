/** The dialog that edits a machine: its name, which is all its request takes, with what the machine reports of itself shown as fixed.
 *
 * A machine is what its connector says it is: its system, its processors, the connector it runs. None of
 * that is changed from here, and the form says so beside each; the name is the one thing that is Orch's own.
 * Whoever opens it as Rename sees the name alone.
 */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminSpec } from '@/ui/AdminAct';

import { renameMachine } from '@/api/resources';
import { EditDialog, Fixed } from '@/features/resources/EditDialog';
import { architectureName, OS_NAMES, readMachineFacts } from '@/features/resources/machines/facts';
import type { Machine } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

export interface EditMachineProps {
  machine: Machine;
  /** `name` shows the name alone, as Rename does. */
  only?: 'name';
  onClose: () => void;
}

export function EditMachine({ machine, only, onClose }: EditMachineProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(machine.name);
  const facts = readMachineFacts(machine.facts);
  const save = useMutation({
    mutationFn: () => renameMachine(machine.id, name.trim()),
    onSuccess: (updated) => {
      toast({ tone: 'success', title: t('resources.edit.saved', { name: updated.name }) });
      onClose();
    },
  });
  return (
    <EditDialog
      admin={adminSpec(machine, ['machine.manage', 'host.manage'])}
      title={only === 'name' ? t('resources.edit.renameTitle', { name: machine.name }) : t('resources.edit.title', { name: machine.name })}
      changed={name.trim() !== machine.name}
      valid={name.trim() !== ''}
      pending={save.isPending}
      error={save.error}
      onSave={() => save.mutate()}
      onClose={onClose}
      {...(only === 'name'
        ? {}
        : {
            rest: (
              <>
                <Fixed label={t('machines.overview.system')} value={`${facts.osVersion ?? OS_NAMES[machine.platform_os]} (${architectureName(machine.platform_arch, t)})`} why={t('machines.edit.reportedWhy')} />
                <Fixed label={t('machines.overview.connector')} value={t('machines.overview.connectorValue', { version: machine.connector_version, protocol: machine.protocol_version })} why={t('machines.edit.connectorWhy')} />
              </>
            ),
          })}
    >
      <Input label={t('resources.edit.name')} value={name} disabled={save.isPending} onChange={(event) => setName(event.target.value)} autoComplete="off" />
    </EditDialog>
  );
}
