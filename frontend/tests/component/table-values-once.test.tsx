/** Table rows show display names once, keep technical identifiers in their editor and retain an existing environment scope. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';

import type * as AdminApi from '@/api/admin';
import type * as UsageApi from '@/api/usage';
import { Capabilities } from '@/features/admin/Capabilities';
import { RateCard } from '@/features/usage/RateCard';
import type { CapabilityDef, RateCatalogue, RateTable } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="#provider">{children}</a> }));
vi.mock('@/api/admin', async (load) => ({ ...await load<typeof AdminApi>(),
  capabilitiesQuery: { queryKey: ['table-value-capabilities'], queryFn: (): Promise<CapabilityDef[]> => Promise.resolve(capabilityRows) },
  capabilitiesPageQuery:(q:string)=>({queryKey:['table-value-capabilities','pages',q],initialPageParam:null as string|null,
   queryFn:({pageParam}:{pageParam:string|null})=>{const matches=capabilityRows.filter((row)=>row.label.toLowerCase().includes(q.toLowerCase()));const from=Number(pageParam??0);return Promise.resolve({items:matches.slice(from,from+50),next_cursor:from+50<matches.length?String(from+50):null});},
   getNextPageParam:(page:{next_cursor:string|null})=>page.next_cursor,
  }),
}));
vi.mock('@/api/usage', async (load) => ({ ...await load<typeof UsageApi>(),
  ratesQuery: () => ({ queryKey: ['table-value-rates'], queryFn: (): Promise<RateTable> => Promise.resolve(rates) }),
  nodeSettingsQuery: () => ({ queryKey: ['table-value-settings'], queryFn: () => Promise.resolve([]) }),
  catalogueQuery: () => ({ queryKey: ['table-value-catalogue'], queryFn: (): Promise<RateCatalogue> => Promise.resolve(catalogue) }),
}));

const capability: CapabilityDef = { key: 'storage_extra', label: 'Extra storage', description: 'Access the extra storage volume.',
  risk: 'medium', default_decision: 'ask', default_approver_scope: 'environment', driver_patch: {}, builtin: false, revision: 1 };
let capabilityRows=[capability];
const provider = { id: 'provider', name: 'Team supply', kind: 'api' as const, harness_kind: null, harness: null,
  address: 'internal.route', placement: 'managed' as const, status: 'ready' as const };
const rates: RateTable = { catalogue: { on: false, scale: '1', as_of: '' }, models: [
  { provider, model_name: 'vendor/internal-route', display_name: 'Readable model', rate: null, source: null, match: null, unrated_tokens: 0 },
  { provider: { ...provider, id: 'another', name: 'Matching name' }, model_name: 'vendor/another-route', display_name: 'Matching name',
    rate: null, source: null, match: null, unrated_tokens: 0 },
] };
const catalogue: RateCatalogue = { format: 1, as_of: '', unit: 'USD', origin: 'shipped', source: null,
  loaded_at: '2026-10-15T00:00:00Z', loaded_by: null, shipped_as_of: '', families: [] };

afterEach(async () => { cleanup();vi.restoreAllMocks();capabilityRows=[capability]; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps a capability's key out of its row, its editor usable, and help in one place (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<div data-theme={theme}><Capabilities /></div>);
    const name = await screen.findByText(capability.label, { exact: true });
    const row = name.closest('tr');
    if (row === null) throw new Error('the capability lost its row');
    expect(within(row).getAllByText(capability.label, { exact: true })).toHaveLength(1);
    expect(within(row).queryByText(capability.key, { exact: true })).not.toBeInTheDocument();
    expect(document.querySelector('[data-help-guide]')).toBeNull();
    await user.click(within(row).getByRole('button', { name: i18n.t('admin.capabilities.menuOf', { name: capability.label }) }));
    await user.click(screen.getByRole('menuitem', { name: i18n.t('admin.capabilities.edit') }));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('admin.capabilities.editTitle', { name: capability.label }) });
    expect(within(dialog).getByLabelText(i18n.t('admin.capabilities.key'))).toHaveValue(capability.key);
    const scope = within(dialog).getByRole('combobox', { name: i18n.t('admin.capabilities.approver') });
    expect(scope).toHaveTextContent(i18n.t('choiceCatalog.approverScope.environment.name'));
    await user.click(scope);
    expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.approverScope.environment.name') })).toBeVisible();
  });

  it(`names each model and provider once without a routing slug (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    show(<div data-theme={theme}><RateCard bare node={{ id: 'org', name: 'Acme', kind: 'org' }} manage={false} /></div>);
    const table = await screen.findByRole('table', { name: i18n.t('usage.rates.table') });
    for (const name of ['Readable model', 'Team supply', 'Matching name']) expect(within(table).getAllByText(name, { exact: true })).toHaveLength(1);
    for (const name of ['vendor/internal-route', 'vendor/another-route', provider.address]) expect(within(table).queryByText(name)).not.toBeInTheDocument();
  });
}

it.each(['en', 'zh-CN'] as const)('asks for the capability name before its technical key in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<Capabilities />);
  await user.click(screen.getByRole('button', { name: i18n.t('admin.capabilities.addTitle') }));
  const dialog = await screen.findByRole('dialog', { name: i18n.t('admin.capabilities.addTitle') });
  expect(within(dialog).getAllByRole('textbox')[0]).toHaveAccessibleName(i18n.t('admin.capabilities.label'));
});


it('finds a later capability through its own native search and keeps an open edit above changed pages',async()=>{
 vi.spyOn(HTMLElement.prototype,'offsetHeight','get').mockImplementation(function(this:HTMLElement){return this.tagName==='TR'?72:384;});
 vi.spyOn(HTMLElement.prototype,'offsetWidth','get').mockReturnValue(600);

 capabilityRows=Array.from({length:1_001},(_,index)=>({...capability,key:`cap_${index}`,label:`Capability ${String(index).padStart(4,'0')}`}));
 const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});show(<Capabilities/>,cache);const user=userEvent.setup();
 await screen.findByText('Capability 0000',{exact:true});expect(screen.queryByText('Capability 1000',{exact:true})).toBeNull();
 const search=screen.getByRole('searchbox',{name:i18n.t('ui.collection.find',{name:i18n.t('admin.sections.capabilities.title')})});
 await user.type(search,'Capability 1000');await screen.findByText('Capability 1000',{exact:true});
 await user.click(screen.getByRole('button',{name:i18n.t('admin.capabilities.menuOf',{name:'Capability 1000'})}));await user.click(screen.getByRole('menuitem',{name:i18n.t('admin.capabilities.edit')}));
 const dialog=screen.getByRole('dialog');const label=within(dialog).getByLabelText(i18n.t('admin.capabilities.label'));await user.clear(label);await user.type(label,'Unfinished edit');
 act(()=>{cache.setQueryData(['table-value-capabilities','pages','Capability 1000'],{pages:[{items:[],next_cursor:null}],pageParams:[null]});});
 expect(dialog).toBeVisible();expect(label).toHaveValue('Unfinished edit');
});
