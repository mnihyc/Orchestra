/** The dialog that deletes an environment: it is named to be sure, and says what goes and what stays, with the threads that depend on it and the refusal the server gives while any are bound.
 *
 * Removing a managed unit keeps its owned volumes for the current frozen window; a directory on a person's
 * machine only loses its registration. What is typed must be its name, as the
 * request asks. Whoever opens it from a list or from the environment's own page gets the same question.
 */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AdminActNotice, adminSpec } from '@/ui/AdminAct';
import { Trash2 } from 'lucide-react';

import { ApiError } from '@/api/client';
import { scheduleEnvironmentRemoval } from '@/api/trace';
import { RemovalNotice } from '@/features/trace/RemovalNotice';
import { useActKey } from '@/lib/forms';
import { environmentLifecycle } from './words';
import type { Environment as EnvironmentRow } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

export function RemoveEnvironmentDialog({ environment, onClose, afterDelete }: { environment: EnvironmentRow; onClose: () => void; afterDelete?: () => void }) {
  const { t } = useTranslation();
  const key = useActKey();
  const lifecycle = environmentLifecycle(environment);
  const [typed, setTyped] = useState('');
  const remove = useMutation({
    mutationFn: () => scheduleEnvironmentRemoval(environment.id, environment.name, key({ id: environment.id, name: environment.name })),
    onSuccess: () => {
      toast({ tone: 'success', title: t('trace.removalScheduled', { name: environment.name }) });
      onClose();
      afterDelete?.();
    },
  });
  const inUse = remove.error instanceof ApiError && remove.error.is('resource.in_use');
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !remove.isPending && onClose()}
      title={t('trace.removeTitle', { name: environment.name })}
      dismissible={!remove.isPending}
      footer={
        <>
          <Button disabled={remove.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" icon={<Trash2 aria-hidden />} pending={remove.isPending} blocked={typed !== environment.name || lifecycle === null} onClick={() => remove.mutate()}>
            {t('trace.removeConfirm', { name: environment.name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-2">
        <AdminActNotice spec={adminSpec(environment, 'environment.manage')} />
        <RemovalNotice environment={environment} />
        {lifecycle === 'unit' && <p className="text-sm text-text-muted">{t('environmentData.purge')}</p>}
        {environment.threads.length > 0 && <InlineAlert tone="warning">{t('environments.remove.threads', { count: environment.threads.length })}</InlineAlert>}
        <Input label={t('environments.remove.type', { name: environment.name })} value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" disabled={remove.isPending} />
        {remove.error !== null && <ErrorAlert error={remove.error} />}
        {inUse && <p className="text-sm text-text-muted">{t('environments.remove.inUse')}</p>}
      </div>
    </Dialog>
  );
}
