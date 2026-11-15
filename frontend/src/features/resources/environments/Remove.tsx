/** The dialog that deletes an environment: it is named to be sure, and says what goes and what stays, with the threads that depend on it and the refusal the server gives while any are bound.
 *
 * Removing a managed unit keeps its owned volumes for the current frozen window; a directory on a person's
 * machine only loses its registration. What is typed must be its name, as the
 * request asks. Whoever opens it from a list or from the environment's own page gets the same question.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AdminActNotice, adminSpec } from '@/ui/AdminAct';
import { Trash2 } from 'lucide-react';

import { ApiError } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import { scheduleEnvironmentRemoval } from '@/api/trace';
import { RemovalNotice } from '@/features/trace/RemovalNotice';
import { useActKey } from '@/lib/forms';
import { environmentLifecycle } from './words';
import type { Environment as EnvironmentRow } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { toast } from '@/ui/Toast';

export function RemoveEnvironmentDialog({ environment: offered, onClose, afterDelete }: { environment: Pick<EnvironmentRow, 'id' | 'name'>; onClose: () => void; afterDelete?: () => void }) {
  const { t } = useTranslation();
  const key = useActKey();
  const current = useQuery({ ...environmentQuery(offered.id), refetchOnMount: 'always' });
  const environment = current.data;
  const name = environment?.name ?? offered.name;
  const lifecycle = environment === undefined ? null : environmentLifecycle(environment);
  const days = environment?.removal?.keep_days ?? environment?.removal_keep_days ?? null;
  const ready = current.isFetchedAfterMount && !current.isFetching && !current.isError && environment !== undefined && lifecycle !== null && (lifecycle !== 'unit' || days !== null);
  const [typed, setTyped] = useState('');
  const remove = useMutation({
    mutationFn: () => {
      if (!ready || environment === undefined) throw new Error(t('trace.removalUnknown'));
      const review = { expected_revision: environment.revision, ...(lifecycle === 'unit' && days !== null ? { expected_keep_days: days } : {}) };
      return scheduleEnvironmentRemoval(environment.id, environment.name, key({ id: environment.id, name: environment.name, review }), review);
    },
    onError: async (error) => {
      if (error instanceof ApiError && error.is('conflict.stale_revision')) {
        setTyped('');
        await current.refetch();
      }
    },
    onSuccess: () => {
      toast({ tone: 'success', title: t('trace.removalScheduled', { name }) });
      onClose();
      afterDelete?.();
    },
  });
  const inUse = remove.error instanceof ApiError && remove.error.is('resource.in_use');
  return (
    <Dialog
      open
      onOpenChange={(next) => !next && !remove.isPending && onClose()}
      title={t('trace.removeTitle', { name })}
      dismissible={!remove.isPending}
      footer={
        <>
          <Button disabled={remove.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="danger" icon={<Trash2 aria-hidden />} pending={remove.isPending} blocked={typed !== name || !ready} reason={!ready ? t('trace.removalUnknown') : undefined} onClick={() => remove.mutate()}>
            {t('trace.removeConfirm', { name })}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 py-2">
        {environment !== undefined && <AdminActNotice spec={adminSpec(environment, 'environment.manage')} />}
        {current.isError ? <ErrorState compact error={current.error} onRetry={() => void current.refetch()} /> : environment === undefined ? <SkeletonRows read={current} rows={3} /> : <RemovalNotice environment={environment} />}
        {lifecycle === 'unit' && <p className="text-sm text-text-muted">{t('environmentData.purge')}</p>}
        {environment !== undefined && environment.threads.length > 0 && <InlineAlert tone="warning">{t('environments.remove.threads', { count: environment.threads.length })}</InlineAlert>}
        <Input label={t('environments.remove.type', { name })} value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" disabled={remove.isPending} />
        {remove.error !== null && <ErrorAlert error={remove.error} />}
        {inUse && <p className="text-sm text-text-muted">{t('environments.remove.inUse')}</p>}
      </div>
    </Dialog>
  );
}
