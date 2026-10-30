/** Providers by what one owns, may use and manages, and all of them for an admin: status, plan, runtime, turns running, and what each is behind in.
 *
 * One request answers four questions. "Mine" lists what the person owns; "I can use" what they may
 * run work on, including what they own and manage; "I manage" what they look after, including what they own; and for an admin
 * of the org in view, "All in" that org, which shows even what they are not named on as far as
 * administering goes. A row says what the provider is ("Subscription · Pro", "API key · 2
 * endpoints"), whose it is, where it runs, how it stands in one phrase with what to do about it, how
 * many turns run on it, how much of its plan is used, and how many models it lists. What is searched
 * for, the view and the status are in the address.
 */
import { Link } from '@tanstack/react-router';
import { PROVIDER_STATUSES, PROVIDER_VIEWS, type ProviderView, type ProvidersSearch } from '@/features/resources/search';
import { Plus, PlugZap } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceReaders } from '@/features/resources/inventory';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { useHarnessTitle } from '@/features/resources/harnesses';
import { useProviderActs } from '@/features/resources/providers/acts';
import { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import { providerState, providerWhat } from '@/features/resources/providers/state';
import { providerLabels } from './labels';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import { oneOf } from '@/features/resources/search';
import type { Provider } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Meter } from '@/ui/Meter';
import { Select } from '@/ui/Select';
import { StatusDot } from '@/ui/StatusDot';
import type { Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { Tooltip } from '@/ui/Tooltip';
import { UpdateDialog } from '@/features/fleet/UpdateDialog';
import { RuntimeChange } from '@/features/resources/TechnicalValue';

export { PROVIDER_STATUSES, PROVIDER_VIEWS, type ProviderView, type ProvidersSearch };

/** How many of a plan's windows a row draws: what fits beside the other columns, and the most urgent are first. */
const WINDOWS_IN_A_ROW = 2;

export function ProviderLink({ provider, label = provider.name }: { provider: Pick<Provider, 'id' | 'name'> & Partial<Provider>; label?: string }) {
  return (
    <span className="flex min-w-0 flex-col gap-0.5"><ResourceName subject={{ kind: 'provider', provider }} asChild><Link to="/resources/providers/$id" params={{ id: provider.id }} className="min-w-0 truncate font-medium text-text hover:underline">
      {label}
    </Link></ResourceName><ResourceHint subject={{ kind: 'provider', provider }} /></span>
  );
}

/** A provider's standing as a dot and one phrase. */
export function ProviderStatusText({ provider, compact = false }: { provider: Provider; compact?: boolean }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const state = providerState(provider, t, { locale, timeZone }, now);
  return compact ? <Tooltip content={state.text}><span tabIndex={0} className="inline-flex max-w-40 rounded-sm"><StatusDot tone={state.tone} label={state.text} showLabel className="min-w-0 [&>span:last-child]:truncate" /></span></Tooltip> : <StatusDot tone={state.tone} label={state.text} showLabel />;
}

/** A unit's connector name is an implementation id; its readable host is where the provider runs. */
export function ProviderLocation({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const host = provider.runtime_support?.lifecycle === 'attach' ? null : provider.runtime?.host;
  if (host) return <ResourceName subject={{ kind: 'host', host }} asChild><Link to="/resources/hosts/$id" params={{ id: host.id }} className="block max-w-36 truncate rounded-sm text-text-muted hover:underline">{host.name}</Link></ResourceName>;
  const machine = provider.runtime_support?.lifecycle === 'unit' ? null : provider.machine;
  if (machine !== null) return <Tooltip content={machine.name}><Link to="/resources/machines/$id" params={{ id: machine.id }} className="block max-w-36 truncate rounded-sm text-text-muted hover:underline">{machine.name}</Link></Tooltip>;
  return <span className="block max-w-36 truncate">{t(provider.machine === null && provider.runtime === null ? 'providers.list.noRuntime' : 'providers.list.locationUnavailable')}</span>;
}

export function ProviderList({ search, onSearch }: { search: ProvidersSearch; onSearch: (patch: Partial<ProvidersSearch>) => void }) {
  const { t } = useTranslation();
  const harnessTitle = useHarnessTitle();
  const [adding, setAdding] = useState(false);
  const acts = useProviderActs();
  const [updating, setUpdating] = useState<string[] | null>(null);
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));

  const places = useKnownPlaces({ trees: false });

  const columns = (rows: readonly Provider[]): Column<Provider>[] => {
    const labels = providerLabels(rows, places);
    return [
    {
      id: 'name',
      header: t('providers.list.name'),
      cell: (provider, shown) => {
        // What a column that is left out of a narrow place would have said is said under the name instead.
        const withoutStatus = !shown.has('status');
        const what = providerWhat(provider, harnessTitle, t);
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex min-w-0 items-center gap-2">
              <ProviderLink provider={provider} label={labels.get(provider.id) ?? provider.name} />
            </span>
            <Tooltip content={what}><span tabIndex={0} className="block min-w-0 truncate rounded-sm text-sm text-text-muted">{what}</span></Tooltip>
            {withoutStatus && <ProviderStatusText compact provider={provider} />}
            {(provider.runtime?.updates ?? []).filter((update) => update.what === 'harness').map((update) => <span key={update.name} className="text-sm wrap-anywhere text-text-muted"><RuntimeChange change={update} /></span>)}
          </div>
        );
      },
    },
    { id: 'owner', header: t('providers.list.owner'), from: 'md', cell: (provider) => <UserChip user={provider.owner} size={24} thing={{ kind: 'provider', id: provider.id }} /> },
    {
      id: 'runs',
      header: t('providers.list.runs'),
      from: 'md',
      className: 'text-text-muted',
      cell: (provider) => <ProviderLocation provider={provider} />,
    },
    {
      id: 'status',
      header: t('providers.list.status'),
      from: 'sm',
      cell: (provider) => <ProviderStatusText compact provider={provider} />,
    },
    {
      id: 'turns',
      header: t('providers.list.turns'),
      from: 'xl',
      className: 'text-right tabular-nums',
      align: 'end',
      cell: (provider) => provider.running_threads,
    },
    {
      id: 'usage',
      header: t('providers.list.usage'),
      from: 'xl',
      cell: (provider) =>
        provider.facts.windows.length === 0 ? (
          <span className="text-text-subtle">{t('providers.list.noUsage')}</span>
        ) : (
          <span className="flex flex-col gap-1">
            {provider.facts.windows.slice(0, WINDOWS_IN_A_ROW).map((window) => (
              <Meter key={window.id} look="compact" value={window.used_percent} label={window.label} />
            ))}
          </span>
        ),
    },
    {
      id: 'models',
      header: t('providers.list.models'),
      from: 'xl',
      align: 'end',
      className: 'text-right tabular-nums',
      cell: (provider) => provider.models.length,
    },
  ]; };

  const add = (
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
      {t('providers.list.add')}
    </Button>
  );

  return (
    <>
      <ResourceList
        kind="providers"
        search={{ ...search }}
        acts={{ ...acts.list, bulk: [{ id: 'update', label: t('change.env.menu.update'), can: () => true, open: (rows) => setUpdating(rows.map((row) => row.id)), done: () => '' }, ...(acts.list.bulk ?? [])], dialogs: <>{acts.list.dialogs}{updating !== null && <UpdateDialog targets={{ environments: [], providers: updating }} onClose={() => setUpdating(null)} />}</> }}
        title={t('screens.providers.title')}
        icon={PlugZap}
        lead={t('providers.list.lead')}
        label={t('providers.list.label')}
        readRow={resourceReaders.providers}
        filter={{ ...(search.view === undefined ? {} : {view:search.view}), state: search.status }}
        columns={columns}
        rowKey={(provider) => provider.id}
        rowClassName={(provider) => (provider.status === 'disabled' ? 'bg-bg-subtle' : undefined)}
        text={text}
        onText={setText}
        searchLabel={t('providers.list.search')}
        searchPlaceholder={t('providers.list.searchPlaceholder')}
        filters={
          <>
            <Select
              label={t('providers.list.statusFilter')}
              size="sm"
              value={search.status ?? 'any'}
              onValueChange={(next) => onSearch({ status: oneOf(PROVIDER_STATUSES, next) })}
              options={[
                { value: 'any', name: t('choiceCatalog.statusFilter.any.name') },
                ...PROVIDER_STATUSES.map((status) => ({ value: status, name: t(`providers.status.${status}`) })),
              ]}
              fieldClassName="w-auto"
            />
          </>
        }
        action={add}
        empty={{
          icon: <PlugZap aria-hidden />,
          title: t('providers.list.none.mine'),
          description: t('providers.list.noneLead.mine'),
          action: add,
        }}
        noMatch={t('providers.list.noMatch')}
      />
      {adding && <AddProviderDialog onClose={() => setAdding(false)} />}
    </>
  );
}
