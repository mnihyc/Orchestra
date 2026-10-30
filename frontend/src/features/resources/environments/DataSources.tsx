/** Independent workspace/home storage choices use actual driver facts and the machine's one environment.roots policy. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { HelpHint } from '@/ui/HelpHint';
import { choiceOption } from '@/i18n/choices';
import { machineEnvironmentDataQuery } from '@/api/resource-tools';
import type { EnvironmentDataSources, EnvironmentDataStorage } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { PathValue } from '@/features/resources/TechnicalValue';

export const madeDataSources = (): EnvironmentDataSources => ({ workspace: { kind: 'made_by_orch' }, home: { kind: 'made_by_orch' } });
export const borrowsData = (value: EnvironmentDataSources): boolean => value.workspace.kind === 'existing_directory' || value.home.kind === 'existing_directory';
const environmentRoles = ['workspace', 'home'] as const;

export function DataSourcesEditor({ machine, driver, value, onChange, disabled = false, onReady, roles = environmentRoles, dataStorage }: {
  machine?: string; driver?: string; value: EnvironmentDataSources; onChange: (value: EnvironmentDataSources) => void; disabled?: boolean; onReady?: (ready: boolean) => void; roles?: readonly ('workspace' | 'home')[]; dataStorage?:EnvironmentDataStorage | null;
}) {
  const { t } = useTranslation();
  const read = useQuery({ ...machineEnvironmentDataQuery(machine ?? ''), enabled: machine !== undefined, retry: false });
  const facts = dataStorage ?? (driver === undefined ? read.data?.direct : read.data?.drivers[driver]);
  const roots = read.data?.roots ?? [];
  const valid = roles.every((role) => {
    const source = value[role];
    return source.kind === 'made_by_orch'
      ? machine === undefined || Boolean(facts?.made_by_orch)
      : facts?.existing_directory === true && source.path.trim() !== '' && roots.some((root) => source.path === root.path || source.path.startsWith(`${root.path.replace(/\/$/, '')}/`));
  });
  useEffect(() => { onReady?.(valid); }, [onReady, valid]);
  const explanation = <HelpHint term={t(roles.length === 1 ? 'environmentData.home' : 'environmentData.title')}
    what={t(roles.length === 1 ? 'environmentData.harnessChoice' : 'environmentData.choices')}
    which={machine === undefined ? t('environmentData.chooseMachine') : undefined} />;
  return <div className="flex flex-col gap-3">
    {machine === undefined ? null : read.isError ? <ErrorState compact error={read.error} onRetry={() => void read.refetch()} />
        : read.isPending ? <SkeletonRows rows={2} /> : null}
    {roles.map((role) => { const source = value[role]; return <div key={role} className="flex flex-col gap-2">
      <Select<'made_by_orch' | 'existing_directory'> label={t(`environmentData.${role}`)} value={value[role].kind}
        help={role === roles[0] ? explanation : undefined}
        disabled={disabled || (machine !== undefined && !read.isSuccess)}
        options={[
          { value: 'made_by_orch', ...choiceOption(t, 'dataSource.made_by_orch'), disabled: machine !== undefined && !facts?.made_by_orch,
            description: machine === undefined ? choiceOption(t, 'dataSource.made_by_orch').description : facts?.made_by_orch === 'directory' ? t('environmentData.newDirectory') : facts?.made_by_orch === 'volume' ? t('environmentData.newVolume') : t('environmentData.notOffered') },
          { value: 'existing_directory', ...choiceOption(t, 'dataSource.existing_directory'), disabled: machine === undefined || facts?.existing_directory !== true || roots.length === 0,
            description: machine === undefined ? t('environmentData.chooseMachine') : facts?.reason ?? (roots.length === 0 ? t('environmentData.noRoots') : t('environmentData.allowed')) },
        ]}
        onValueChange={(kind) => onChange({ ...value, [role]: kind === 'made_by_orch' ? { kind } : { kind, path: roots[0]?.path ?? '' } })} />
      {source.kind === 'existing_directory' && <>
        <Input label={t('environmentData.path')} value={source.path} disabled={disabled}
          error={source.path.trim() !== '' && !roots.some((root) => source.path === root.path || source.path.startsWith(`${root.path.replace(/\/$/, '')}/`)) ? t('environmentData.chooseAllowed') : undefined}
          onChange={(event) => onChange({ ...value, [role]: { kind: 'existing_directory', path: event.target.value } })} mono />
      </>}
    </div>; })}
    {roles.some((role) => value[role].kind === 'existing_directory') && <>
      <p className="text-sm text-text-muted">{t('environmentData.allowed')}: {roots.map((root) => root.path).join(', ')}</p>
      <p className="text-sm text-text-muted">{t('environmentData.preserved')}</p>
    </>}
  </div>;
}

export function DataSourcesSummary({ value }: { value?: EnvironmentDataSources | null }) {
  const { t } = useTranslation();
  if (value === undefined || value === null) return null;
  return <Section id="environment-data-sources" title={t('environmentData.title')}>
    <dl>{environmentRoles.map((role) => { const source = value[role]; return <div key={role} className="flex flex-wrap gap-2">
      <dt>{t(`environmentData.${role}`)}</dt><dd>{source.kind === 'existing_directory' ? <PathValue path={source.path} /> : t('environmentData.made')}</dd>
    </div>; })}</dl>
    {borrowsData(value) && <p className="text-sm text-text-muted">{t('environmentData.preserved')}</p>}
  </Section>;
}

/** One served projection is reused by the host and personal machine pages. */
export function EnvironmentDirectoriesHere({ machine }: { machine: string }) {
  const { t } = useTranslation();
  const read = useQuery(machineEnvironmentDataQuery(machine));
  return <Section id="machine-environment-directories" title={t('environmentData.roots')} lead={t('environmentData.policy', { setting: t('machines.policy.keys.environment_roots.title') })}>
    {read.isError ? <ErrorState compact error={read.error} onRetry={() => void read.refetch()} /> : read.isPending ? <SkeletonRows rows={2} /> : <>
      {read.data.roots.length === 0 ? <EmptyState compact title={t('environmentData.noRoots')} /> : <ul className="flex flex-col gap-3">
        {read.data.roots.map((root) => <li key={root.path}><code>{root.path}</code>
          <ul>{root.uses.map((use) => <li key={`${use.environment.id}:${use.role}`} className="flex flex-wrap gap-x-3 gap-y-1">
            <Link to="/resources/environments/$id" params={{ id: use.environment.id }}>{use.environment.name}</Link>
            <span>{t(`environmentData.${use.role}`)}</span><code>{use.path}</code>
          </li>)}</ul>
        </li>)}
      </ul>}
      <Link to="/resources/machines/$id" params={{ id: machine }} search={{ tab: 'policy' }}>{t('environmentData.editPolicy')}</Link>
    </>}
  </Section>;
}
