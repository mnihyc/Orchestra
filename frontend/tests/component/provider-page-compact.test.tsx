/** Six visible sections, one setup act and one compact unread statement; inactive views do not mount. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { ProviderScreen, providerTab, type ProviderTab } from '@/features/resources/providers/Provider';
import type { PageHeaderProps } from '@/shell/Page';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { FACT_MEMBERS } from '@/features/resources/providers/details';
import { show } from './support';

const panel=vi.hoisted(()=>vi.fn<(name:string)=>void>());
const provider=planProvider({status:'needs_setup'});
provider.facts.credential=null;
for(const member of FACT_MEMBERS)provider.facts_state[member]='not_read_yet';
provider.facts.available_resets=null;
vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),providerQuery:()=>({queryKey:['compact-provider'],queryFn:()=>Promise.resolve(provider)})}));
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>vi.fn(),Link:({children}:{children:ReactNode})=><a href="#resource">{children}</a>}));
vi.mock('@/features/resources/trail',()=>({useResourceHeading:()=>undefined}));
vi.mock('@/features/resources/providers/acts',()=>({useProviderActs:()=>({page:()=>({primary:null,edit:null,menu:[]}),dialogs:null})}));
vi.mock('@/shell/Page',async(load)=>({...await load<object>(),Page:({children}:{children:ReactNode})=><main>{children}</main>,PageHeader:({title,actions}:PageHeaderProps)=><header><h1>{title}</h1>{actions}</header>}));
vi.mock('@/features/resources/providers/Models',()=>({ProviderModels:()=>{panel('models');return null;}}));
vi.mock('@/features/resources/providers/Key',()=>({ProviderKey:()=>{panel('key');return null;}}));
vi.mock('@/features/resources/providers/Usage',()=>({ProviderUsage:()=>{panel('usage');return null;}}));
vi.mock('@/features/resources/providers/Runtime',()=>({ProviderRuntime:()=>{panel('runtime');return null;}}));
vi.mock('@/features/resources/providers/Settings',()=>({ProviderSettings:()=>{panel('settings');return null;}}));
vi.mock('@/features/resources/providers/Arrangements',()=>({ProviderArrangements:()=>null}));
vi.mock('@/features/resources/providers/DangerZone',()=>({ProviderDangerZone:()=>null}));
vi.mock('@/features/resources/ResourceAccess',()=>({ResourceAccess:()=>null}));
vi.mock('@/features/resources/providers/Threads',()=>({ProviderThreads:()=>{panel('activity');return null;}}));
vi.mock('@/features/resources/providers/Reset',()=>({ProviderReset:()=>null}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
function Page(){const [tab,setTab]=useState<ProviderTab>('overview');return <ProviderScreen providerId={provider.id} tab={tab} onTab={setTab}/>;}
beforeEach(()=>panel.mockClear());
afterEach(async()=>{document.documentElement.removeAttribute('data-theme');await setLocale('en');});
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])describe(`${locale}/${theme}`,()=>{
  it('shows one action per task and loads a section only when selected',async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    const t=i18n.getFixedT(locale);const user=userEvent.setup();
    show(<Page/>);
    await screen.findByRole('heading',{level:1,name:provider.name});
    const tabs=screen.getByRole('tablist');
    expect(within(tabs).getAllByRole('tab')).toHaveLength(6);
    for(const key of ['overview','models','credentials','usage','settings','activity'] as const)
      expect(within(tabs).getByRole('tab',{name:(name)=>name===t(`providers.tabs.${key}`)||name.startsWith(t(`providers.tabs.${key}`)+' ')})).toBeVisible();
    expect(screen.queryByRole('button',{name:t('resourceCreation.providers')})).not.toBeInTheDocument();
    expect(screen.getAllByRole('button',{name:t('resources.act.nextSignIn')})).toHaveLength(1);
    expect(screen.getAllByText(t('providerDetails.notReadOnce'),{exact:false})).toHaveLength(1);
    expect(screen.getAllByText(t('providerDetails.members.identity'),{exact:true})).toHaveLength(1);
    expect(panel).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button',{name:t('resources.act.nextSignIn')}));
    expect(panel).toHaveBeenCalledWith('key');
    expect(panel).not.toHaveBeenCalledWith('usage');
    expect(panel).not.toHaveBeenCalledWith('settings');
    panel.mockClear();
    await user.click(within(tabs).getByRole('tab',{name:t('providers.tabs.settings')}));
    expect(panel).toHaveBeenCalledWith('runtime');expect(panel).toHaveBeenCalledWith('settings');
    expect(panel).not.toHaveBeenCalledWith('models');
  });
});
it('keeps every legacy section bookmark reachable through the six sections',()=>{
  for(const name of ['runtime','arrangements','access','danger'])expect(providerTab(name)).toBe('settings');
  for(const name of ['threads','sessions'])expect(providerTab(name)).toBe('activity');
  expect(providerTab('unknown')).toBeUndefined();
});
