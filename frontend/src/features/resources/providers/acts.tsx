/** What can be done with a provider, from its row in the list and from its own page: take the next step of setting it up, open, switch on or off, edit, rename, copy its link and delete, each only where the reader may.
 *
 * The same acts serve the button and the menu at the end of a row, the menu under the right mouse button, the
 * acts rows that are ticked share, and the menu and the Edit button in the heading of the page. Switching
 * off and deleting ask first, naming the provider, with the dialogs the page already has; running turns are interrupted at once while bound threads keep their history and queued inputs.
 * Switching those threads is a separate act.
 */
import { openChange } from '@/features/change/target';
import { useResourceCleanup } from '@/features/storage/acts';
import { useResourceEnable, type EnableResource } from '@/features/storage/Enable';
import { adminSpec } from '@/ui/AdminAct';
import { useNavigate } from '@tanstack/react-router';
import { ArrowRightLeft, Ban, CirclePlay, Copy, ArrowRight, KeyRound, TextCursorInput, Trash2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { deleteProvider, disableProvider, enableProvider } from '@/api/resources';
import { type Act, type BulkAct, BulkNames, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink, shareAct } from '@/features/resources/acts';
import { DeleteProviderDialog, DisableProviderDialog } from '@/features/resources/providers/DangerZone';
import { EditProvider } from '@/features/resources/providers/Edit';
import type { ProviderTab } from '@/features/resources/providers/Provider';
import { PROVIDER_NEXT, providerState } from '@/features/resources/providers/state';
import type { Provider } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { useNow } from '@/lib/time';

export interface Asked {
  what: 'edit' | 'rename' | 'disable' | 'delete';
  provider: Provider;
}

export interface ProviderActs {
  list: ListActs<Provider>;
  /** Asks for an act that has a dialog, for a button of the page that is not in the menu. */
  ask: (what: Asked['what'], provider: Provider) => void;
  page: (provider: Provider) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  dialogs: ReactNode;
}

/** The page of a provider says how its tabs are opened. */
export function useProviderActs(options: { onTab?: (provider: Provider, tab: ProviderTab) => void; afterDelete?: () => void } = {}): ProviderActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const copyLink = useCopyLink();
  const { busy } = useBusy();
  const cleanup = useResourceCleanup();
  const enablePlan = useResourceEnable();
  const [asked, setAsked] = useState<Asked | null>(null);

  /** Leads to a tab of the provider's page: from a row by the address, from the page by its own way. */
  const toTab = (provider: Provider, tab: ProviderTab) => {
    if (options.onTab !== undefined) options.onTab(provider, tab);
    else void navigate({ to: '/resources/providers/$id', params: { id: provider.id }, search: tab === 'overview' ? {} : { tab } });
  };
  const enableResource = (provider: Provider): EnableResource => ({target:{object:{kind:'provider',id:provider.id},model_id:null},name:provider.name,admin:adminSpec(provider,'provider.manage'),enable:(runWaiting)=>enableProvider(provider.id,runWaiting)});
  const turnOff = (provider: Provider): Promise<unknown> => disableProvider(provider.id);
  const drop = (provider: Provider): Promise<void> => deleteProvider(provider.id);

  const actsOf = (provider: Provider, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manage = holds(provider.access, 'provider.manage');
    const state = providerState(provider, t, { locale, timeZone }, now);
    const step = state.next === null ? null : PROVIDER_NEXT[state.next];
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/providers/$id', params: { id: provider.id } }) } : null;
    // The next step of setting it up, which is the act most wanted of a provider that is not ready.
    const next: Act | null =
      manage && step !== null && where === 'row'
        ? {
            id: 'next',
            label: t(step.label),
            icon: <KeyRound aria-hidden />,
            run: () => toTab(provider, step.tab),
          }
        : null;
    const enable: Act | null = manage && provider.status === 'disabled' ? { id: 'enable', label: t('resources.act.enable'), icon: <CirclePlay aria-hidden />, run: () => enablePlan.ask(enableResource(provider)) } : null;
    const disable: Act | null = manage && provider.status !== 'disabled' ? { id: 'disable', label: t('resources.act.disable'), icon: <Ban aria-hidden />, run: () => setAsked({ what: 'disable', provider }) } : null;
    const rename: Act | null = manage ? { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => setAsked({ what: 'rename', provider }) } : null;
    const share = shareAct(t, { kind: 'provider', id: provider.id, name: provider.name }, provider.access);
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/providers/${provider.id}`) };
    const switchThreads: Act = {id:'switch-threads',label:t('resources.act.switchThreads'),icon:<ArrowRightLeft aria-hidden />,run:()=>openChange({kind:'provider',id:provider.id,name:provider.name},{kind:'switch'})};
    const del: Act | null = manage ? { id: 'delete', label: t('resources.act.delete'), icon: <Trash2 aria-hidden />, danger: true, run: () => setAsked({ what: 'delete', provider }) } : null;
    return {
      primary: next ?? enable ?? open,
      edit: null,
      entries: entriesOf([open], where === 'row' ? [next, enable, disable] : [], [rename], [switchThreads], [share, copy], [del], [cleanup.act({kind:'provider',id:provider.id}, manage)]),
      keys: keysOf(rename),
      busy: busy(provider.id),
    };
  };

  const bulk: BulkAct<Provider>[] = [
    {
      id: 'enable',
      label: t('resources.act.enable'),
      icon: <CirclePlay aria-hidden />,
      can: (provider) => holds(provider.access, 'provider.manage') && provider.status === 'disabled',
      open: (rows) => enablePlan.askMany(rows.map(enableResource)),
      done: (count) => t('providers.bulk.enabled', { count }),
    },
    {
      id: 'disable',
      label: t('resources.act.disable'),
      icon: <Ban aria-hidden />,
      can: (provider) => holds(provider.access, 'provider.manage') && provider.status !== 'disabled',
      run: turnOff,
      confirm: (rows) => ({
        title: t('providers.bulk.disableTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('providers.bulk.disableBody')} />,
        label: t('providers.bulk.disableConfirm', { count: rows.length }),
      }),
      done: (count) => t('providers.bulk.disabled', { count }),
    },
    {
      id: 'delete',
      label: t('resources.act.delete'),
      icon: <Trash2 aria-hidden />,
      danger: true,
      can: (provider) => holds(provider.access, 'provider.manage'),
      run: drop,
      confirm: (rows) => ({
        title: t('providers.bulk.deleteTitle', { count: rows.length }),
        body: <BulkNames names={rows.map((row) => row.name)} lead={t('providers.danger.deleteBody')} />,
        label: t('providers.bulk.deleteConfirm', { count: rows.length }),
      }),
      done: (count) => t('providers.bulk.deleted', { count }),
    },
  ];

  const dialogs = (
    <>
      {cleanup.dialogs}
      {enablePlan.dialogs}
      {asked !== null && (asked.what === 'edit' || asked.what === 'rename') && (
        <EditProvider
          key={`${asked.provider.id}-${asked.what}`}
          provider={asked.provider}
          {...(asked.what === 'rename' ? { only: 'name' as const } : {})}
          onTab={(tab) => toTab(asked.provider, tab)}
          onClose={() => setAsked(null)}
        />
      )}
      {asked?.what === 'disable' && <DisableProviderDialog provider={asked.provider} open onClose={() => setAsked(null)} />}
      {asked?.what === 'delete' && <DeleteProviderDialog provider={asked.provider} open onClose={() => setAsked(null)} {...(options.afterDelete === undefined ? {} : { afterDelete: options.afterDelete })} />}
    </>
  );

  return {
    list: { name: (provider) => provider.name, menuName: (provider) => provider.name, of: (provider) => actsOf(provider, 'row'), bulk, dialogs },
    ask: (what, provider) => setAsked({ what, provider }),
    page: (provider) => {
      const acts = actsOf(provider, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
