/** A retained environment restored directly from its audit row through current metadata and the actual job. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { restoreEnvironment, traceJson } from '@/api/trace';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { JobProgress } from './KeptEnvironments';
import type { Environment as RetainedEnvironment } from '@/generated/api-types';

export function EnvironmentRestore({ id, allowed }: { id: string; allowed: boolean }) {
  const { t } = useTranslation(); const cache = useQueryClient(); const [job, setJob] = useState<string | null>(null);
  const restore = useMutation({ mutationFn: async () => {
    const current = await traceJson<RetainedEnvironment>(`/environments/${id}?inspect_deleted=true`);
    if (current.removal?.can_restore !== true) { if (current.removal?.job !== null && current.removal?.job !== undefined) setJob(current.removal.job.id); throw new Error('the current restoration act is unavailable'); }
    return restoreEnvironment(id, current.removal.revision, newActKey());
  }, onSuccess: (accepted) => setJob(accepted.id), onSettled: (_data, error) => { if (error !== null) { void cache.invalidateQueries({ queryKey: ['trace'] }); void cache.invalidateQueries({ queryKey: ['trace-deleted'] }); } } });
  return <div className="flex flex-col gap-2">{allowed && <div><Button size="sm" icon={<RotateCcw aria-hidden />} pending={restore.isPending} onClick={() => restore.mutate()}>{t('trace.restore')}</Button></div>}{restore.error !== null && <ErrorAlert error={restore.error} {...(restore.error instanceof ApiError ? {} : { message: t('trace.restoreUnavailable') })} />}{job !== null && <JobProgress id={job} environmentId={id} />}</div>;
}
