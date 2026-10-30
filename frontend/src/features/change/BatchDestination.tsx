/** One destination environment, existing or made once, for every member of a switch batch. */
import { Hint } from '@/features/help/Hint';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { environmentsQuery } from '@/api/environments';
import { hostsQuery } from '@/api/resources';
import type { ForkEnvironment } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';

export function BatchDestination({ value, onChange, sources = [], allowSame = true }: { value: ForkEnvironment | undefined; onChange: (next: ForkEnvironment | undefined) => void; sources?: readonly string[]; allowSame?: boolean }) {
  const { t } = useTranslation();
  const environments = useQuery(environmentsQuery(null));
  const hosts = useQuery({ ...hostsQuery(null), enabled: value !== undefined && 'new' in value });
  const mode = value === undefined || 'same' in value ? (allowSame ? 'keep' : '') : 'new' in value ? 'new' : `use:${value.use}`;
  return <div className="flex min-w-0 flex-col gap-2" data-batch-destination>
    <Select help={<Hint popover id="loss-switch" />} label={t('loss.destination')} value={mode} placeholder={t('changeCompact.chooseEnvironment')} onValueChange={(next) => onChange(next === 'keep' || next === '' ? undefined : next === 'new' ? { new: { host_id: null } } : { use: next.slice('use:'.length) })} options={[
      ...(allowSame ? [{ value: 'keep', name: t('loss.keepEnvironment') }] : []),
      { value: 'new', name: t('loss.newEnvironment') },
      ...(environments.data ?? []).filter((environment) => !sources.includes(environment.id) && !['lost', 'removed', 'disabled'].includes(environment.status) && holds(environment.access, 'environment.manage')).map((environment) => ({ value: `use:${environment.id}`, name: environment.name, ...resourceChoice({ kind: 'environment', environment }) })),
    ]} />
    {environments.error !== null && <ErrorAlert error={environments.error} />}
    {value !== undefined && 'new' in value && <div className="grid gap-2 sm:grid-cols-2">
      <Input label={t('change.where.name')} optional value={value.new.name ?? ''} onChange={(event) => onChange({ new: { ...value.new, name: event.target.value } })} />
      <Select label={t('change.where.place')} value={value.new.host_id ?? 'automatic'} onValueChange={(id) => onChange({ new: { ...value.new, host_id: id === 'automatic' ? null : id } })} options={[{ value: 'automatic', name: t('changeCompact.automatic') }, ...(hosts.data ?? []).filter((host) => host.machine.online && host.lost_at === undefined).map((host) => ({ value: host.id, name: host.name, ...resourceChoice({ kind: 'host', host }) }))]} />
      {hosts.error !== null && <ErrorAlert error={hosts.error} />}
    </div>}
  </div>;
}
