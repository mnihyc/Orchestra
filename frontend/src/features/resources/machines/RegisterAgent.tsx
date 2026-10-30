/** The dialog that registers an agent profile for a harness found on a machine: which harness, what the profile is called and which org it belongs to.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { harnessKindsQuery, registerAgent } from '@/api/resources';
import { readMachineFacts, registrableHarnesses } from '@/features/resources/machines/facts';
import { HomePicker } from '@/features/resources/HomePicker';
import { useActKey, useForm } from '@/lib/forms';
import type { Machine } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { toast } from '@/ui/Toast';

export function RegisterAgentDialog({ machine, open, onClose }: { machine: Machine; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const kinds = useQuery({ ...harnessKindsQuery, enabled: open });
  const harnesses = registrableHarnesses(readMachineFacts(machine.facts));
  const [kind, setKind] = useState(harnesses[0]?.kind ?? '');
  const [home, setHome] = useState('');
  const key = useActKey();
  // Whether the form took the server's complaint to one of its fields; if not, it is said as a whole.
  const [placed, setPlaced] = useState(false);
  const titleOf = (value: string) => harnesses.find((entry)=>entry.kind===value)?.title ?? kinds.data?.find((entry) => entry.kind === value)?.title ?? value;
  const form = useForm({ name: '' }, { name: (value) => (value.trim() === '' && kind === '' ? t('machines.agent.nameEmpty') : null) });
  const proposed = kind === '' ? '' : t('machines.agent.proposed', { harness: titleOf(kind), machine: machine.name });

  const register = useMutation({
    mutationFn: (name: string) => {
      const body = { home_id: home, harness_kind: kind, name: name.trim() === '' ? proposed : name.trim() };
      return registerAgent(machine.id, body, key(body));
    },
    onSuccess: (agent) => {
      toast({ tone: 'success', title: t('machines.agent.done', { name: agent.name }) });
      onClose();
      void navigate({ to: '/resources/agents/$id', params: { id: agent.id } });
    },
    onError: (error) => setPlaced(form.refuse(error)),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !register.isPending && onClose()}
      title={t('machines.agent.title', { machine: machine.name })}
      description={t('machines.agent.lead')}
      dismissible={!register.isPending}
      footer={
        <>
          <Button disabled={register.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" type="submit" form="register-agent" pending={register.isPending} blocked={kind === '' || home === ''}>
            {t('machines.agent.submit')}
          </Button>
        </>
      }
    >
      <form id="register-agent" className="flex flex-col gap-4 py-2" noValidate onSubmit={form.submit((values) => kind !== '' && home !== '' && register.mutate(values.name))}>
        <ChoiceLocator
          label={t('machines.agent.harness')}
          hint={harnesses.length === 0 ? t('machines.agent.noHarness') : t('machines.agent.harnessHint')}
          value={kind}
          onValueChange={setKind}
          disabled={harnesses.length === 0 || register.isPending}
          placeholder={harnesses.length === 0 ? t('machines.agent.noHarness') : undefined}
          options={harnesses.map((harness) => ({
            value: harness.kind,
            label: t('machines.agent.harnessOption', { harness: titleOf(harness.kind), version: harness.version }),
            description: harness.path,
          }))}
        />
        <Input label={t('machines.agent.name')} optional hint={t('machines.agent.nameHint', { proposed })} {...form.field('name')} autoComplete="off" />
        <HomePicker value={home} onChange={setHome} />
        {register.error !== null && !placed && <ErrorAlert error={register.error} />}
      </form>
    </Dialog>
  );
}
