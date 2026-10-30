/** Compact access choices, explanations on intent, one focus switch and a defaults action. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as AccessApi from '@/api/access';
import type * as AuthApi from '@/api/auth';
import type * as NodesApi from '@/api/nodes';
import { AccessRequest } from '@/features/access/AccessRequest';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { Roles } from '@/features/access/Roles';
import { RolesAndDefaults } from '@/features/settings/org/RolesAndDefaults';
import type { MissingAccess, NodeRoleListReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { actionName } from '@/ui/actions';
import { me, org } from '../doubles/fixtures';
import { ADA, CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls=vi.hoisted(()=>({ request:vi.fn<typeof AccessApi.requestAccess>(), defaults:vi.fn() }));
const place=org('00000000-0000-4000-8000-000000000001','acme','Acme',ADA);
const modes=[['en','light'],['en','dark'],['zh-CN','light'],['zh-CN','dark']] as const;
const before=useAppearance.getState();
const previousTheme=document.documentElement.dataset.theme;
const wasDark=document.documentElement.classList.contains('dark');

vi.mock('@/api/access',async(original)=>({ ...await original<typeof AccessApi>(),
  permissionsQuery:{queryKey:['compact-permissions'],queryFn:()=>Promise.resolve(CATALOGUE.reply)},
  defaultsQuery:(id:string)=>({queryKey:['compact-defaults',id],queryFn:()=>Promise.resolve(pageFor('org'))}),
  rolesQuery:(id:string)=>({queryKey:['compact-roles',id],queryFn:():Promise<NodeRoleListReply>=>Promise.resolve({owners:[],own:[],above:[],can_give:[]})}),
  requestAccess:(...args:Parameters<typeof AccessApi.requestAccess>)=>calls.request(...args),
  accessRequestQuery:(batch:string)=>({queryKey:['compact-request',batch],queryFn:()=>Promise.resolve({batch_id:batch,approvals:[],complete:true})}),
}));
vi.mock('@/api/auth',async(original)=>({ ...await original<typeof AuthApi>(),
  meQuery:{queryKey:['compact-me'],queryFn:()=>Promise.resolve(me(1,'ada','Ada'))},
}));
vi.mock('@/api/nodes',async(original)=>({ ...await original<typeof NodesApi>(),
  nodeQuery:(id:string)=>({queryKey:['compact-node',id],queryFn:()=>Promise.resolve(place)}),
}));
vi.mock('@/features/access/AccessDialog',()=>({openAccess:(...args:unknown[])=>{calls.defaults(...args);} }));
vi.mock('@/features/navigation/placeLabels',()=>({usePlaceLabel:()=> (node:{name?:string|null}|null|undefined)=>node?.name??null}));
vi.mock('@/features/trace/Person',()=>({useTraceEntry:()=>()=>[],TracePerson:()=>null}));

beforeEach(()=>{
  vi.clearAllMocks();
  sessionStorage.clear();
  calls.request.mockResolvedValue({batch_id:'00000000-0000-4000-8000-000000000002',approvals:[]});
});
afterEach(async()=>{
  cleanup();
  useAppearance.setState(before);
  document.documentElement.classList.toggle('dark',wasDark);
  if(previousTheme===undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme=previousTheme;
  await setLocale('en');
});

async function appearance(locale:'en'|'zh-CN',theme:'light'|'dark') {
  await setLocale(locale);
  useAppearance.setState({theme,resolvedTheme:theme});
  document.documentElement.dataset.theme=theme;
  document.documentElement.classList.toggle('dark',theme==='dark');
}

it.each(modes)('keeps access names distinct, explanations visible and all options in one question in %s/%s',async(locale,theme)=>{
  await appearance(locale,theme);
  const user=userEvent.setup();
  const missing:MissingAccess={object:{kind:'thread',id:'00000000-0000-4000-8000-000000000003',name:'Example',owner:ADA},
    permissions:['thread.view'],ask:[ADA],ask_more:0,closed:null,options:null,override:false};
  const view=show(<AccessRequest objectId={missing.object.id} missing={[missing]} />);
  const names=locale==='en'?['View','Interact','Manage']:['查看','参与','管理'];
  const group=await screen.findByRole('radiogroup',{name:i18n.t('access.request.choose')});
  expect(screen.getAllByText(i18n.t('access.request.send'))).toHaveLength(1);
  expect(screen.queryByRole('heading',{name:i18n.t('access.request.title')})).not.toBeInTheDocument();
  await waitFor(()=>expect(within(group).getAllByRole('radio')).toHaveLength(names.length));
  for (const name of names) expect(within(group).getByRole('radio',{name})).toHaveAccessibleName(name);
  expect(within(group).getByRole('radio',{name:names[0]})).toBeChecked();
  expect(within(group).getByText(i18n.t('adminReach.requestChoices.full_view'))).toBeVisible();
  expect(view.container.querySelector('[data-choice-selected-description]')).toBeNull();
  expect(within(group).getByText(i18n.t('adminReach.requestChoices.full_view'))).toBeVisible();
  await user.click(within(group).getByRole('radio',{name:names[1]}));
  expect(within(group).getByText(i18n.t('adminReach.requestChoices.full_view'))).toBeVisible();
  expect(within(group).getByRole('radio',{name:names[1]})).toHaveAccessibleDescription(i18n.t('adminReach.requestChoices.full_interact'));
  await user.click(within(group).getByRole('radio',{name:names[2]}));
  expect(view.container.querySelector('[data-choice-selected-description]')).toBeNull();
  expect(within(group).getByRole('radio',{name:names[2]})).toHaveAccessibleDescription(i18n.t('adminReach.requestChoices.full_manage'));
  await user.click(screen.getByRole('button',{name:i18n.t('help.hint.label',{term:i18n.t('access.request.choose')})}));
  const help=screen.getByRole('dialog',{name:i18n.t('access.request.choose')});
  for(const choice of ['full_view','full_interact','full_manage'] as const)expect(help).toHaveTextContent(i18n.t(`adminReach.requestChoices.${choice}`));
  expect(calls.request).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  await user.type(screen.getByRole('textbox',{name:i18n.t('access.request.reason')}),'Need to work together');
  await user.click(screen.getByRole('button',{name:i18n.t('access.request.send')}));
  await waitFor(()=>expect(calls.request).toHaveBeenCalledTimes(1));
  expect(calls.request.mock.calls[0]?.[0]).toMatchObject({reason:'Need to work together',items:[{
    object:{kind:'thread',id:missing.object.id},permissions:CATALOGUE.choice('full_manage','thread'),
  }]});
});

it.each(modes)('uses one compact on-page participation switch without a paragraph in %s/%s',async(locale,theme)=>{
  await appearance(locale,theme);
  function Focus(){const[checked,setChecked]=useState(false);return <ParticipationFilter checked={checked} onCheckedChange={setChecked}/>;}
  const view=show(<Focus/>);
  const control=screen.getByRole('switch',{name:i18n.t('adminReach.focus.all')});
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  expect(control).not.toBeChecked();
  expect(control).not.toHaveAccessibleDescription();
  expect(view.container.querySelector('p')).toBeNull();
  await userEvent.click(control);
  expect(control).toBeChecked();
  expect(control).toHaveAccessibleName(i18n.t('adminReach.focus.only'));
});

it.each(modes)('keeps View defaults once in the page and once in a separate inline Roles host in %s/%s',async(locale,theme)=>{
  await appearance(locale,theme);
  const view=show(<RolesAndDefaults org={place}/>);
  const name=actionName('defaults');
  await screen.findByText(i18n.t('access.roles.none'));
  expect(screen.getAllByRole('button',{name})).toHaveLength(1);
  await userEvent.click(screen.getByRole('button',{name}));
  expect(calls.defaults).toHaveBeenCalledWith({kind:'org',id:place.id,name:place.name},{tab:'defaults',share:false});
  view.unmount();
  const open=vi.fn();
  show(<Roles place={{kind:'org',id:place.id,name:place.name}} onDefaults={open}/>);
  const action=await screen.findByRole('button',{name});
  expect(screen.getAllByRole('button',{name})).toHaveLength(1);
  await userEvent.click(action);
  expect(open).toHaveBeenCalledTimes(1);
});
