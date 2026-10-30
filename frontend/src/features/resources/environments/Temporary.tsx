/** A temporary environment is an explicit choice; the actual deadline and Keep come from its returned resource. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { environmentKey, keepEnvironment } from '@/api/environments';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { useAppearance } from '@/lib/appearance';
import type { Environment } from '@/generated/api-types';
import { formatDateTime, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Switch } from '@/ui/Switch';
import { holds } from '@/lib/permissions';
import { EnvironmentExportButton } from './WorkspaceExport';
import { environmentRemovalPhase } from './words';

/** Destructive consent is in the switch's tooltip; the one guide explains its lifetime. */
export function TemporaryChoice({ checked, onChange, disabled = false }: { checked: boolean; onChange: (value: boolean) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  return <div className="flex items-start gap-1">
    <Switch name={t('temporary.name')} description={t('temporary.description')} checked={checked} onCheckedChange={onChange} disabled={disabled}
      tooltip={checked ? t('temporary.consent') : t('temporary.description')} />
    <TaskGuideLink guide="temporary-environments" />
  </div>;
}

/** Keep makes the returned environment lasting in one request. */
export function KeepEnvironmentButton({ id, onKept }: { id: string; onKept?: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const keep = useMutation({ mutationFn: () => keepEnvironment(id), onSuccess: (environment) => {
    cache.setQueryData(environmentKey(id), environment);
    onKept?.();
  } });
  return <span className="inline-flex flex-col gap-1">
    <Button size="sm" pending={keep.isPending} onClick={() => keep.mutate()}>{t('temporary.keep')}</Button>
    {keep.error !== null && <ErrorAlert error={keep.error} />}
  </span>;
}

/** One compact lifetime row, independent of which resource tab is open. */
export function TemporaryStatus({ environment }: { environment: Pick<Environment, 'id' | 'name' | 'temporary' | 'access' | 'removal'> & Partial<Pick<Environment, 'job'>> }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const lifetime = environment.temporary;
  // Once removal is accepted, the page/list's native lifecycle status owns that state once.
  if (lifetime === null || lifetime === undefined || environmentRemovalPhase(environment) !== undefined) return null;
  const when = lifetime.expires_at === null ? t('temporary.afterIdle', { count: lifetime.idle_hours }) : formatDateTime(lifetime.expires_at, { locale, timeZone }, now);
  const manage = holds(environment.access, 'environment.manage');
  return <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm" data-temporary-environment>
    <span>{t(lifetime.on_archive ? 'temporary.statusThread' : 'temporary.status', { when })}</span>
    <TaskGuideLink guide="temporary-environments" />
    {manage && <><KeepEnvironmentButton id={environment.id} /><EnvironmentExportButton id={environment.id} name={environment.name} /></>}
  </div>;
}
