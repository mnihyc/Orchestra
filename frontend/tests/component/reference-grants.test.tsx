/** All four quick grants use the server clock, and the same card handles a late answer and a group membership. */
import {cleanup,screen,waitFor,within} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import type * as Api from '@/api/approvals';
import {ApiError} from '@/api/client';
import {AccessCard} from '@/features/approvals/Access';
import {AccessRequest} from '@/features/access/AccessRequest';
import type * as Access from '@/api/access';
import type {Approval,UserRef} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {show} from './support';
import {formatRelative} from '@/lib/time';
import {useAppearance} from '@/lib/appearance';
const ada={id:'ada',kind:'human',username:'ada',display_name:'Ada',avatar:{text:'A',color:2}} as const;
const bo={...ada,id:'bo',username:'bo',display_name:'Bo'};
const approval={id:'join',kind:'access',state:'pending',title:'Join the group',node:null,object:{kind:'chat',id:'group',name:'Team chat'},requester:{user:bo},request:{membership:true,label:'Shared group',permissions:[],options:null,holder:{kind:'user',user:bo},reason:'Collaborating',for_thread:null},can_decide:true,quick_grant_durations:['1d','7d','30d','always'],decided_at:null,decided_by:null,decision:null,created_at:'2026-10-26T12:00:00Z',expires_at:null,item_id:null,thread_id:null,batch_id:'batch',required_permission:'',approver_scope:null,decided_automatically:false} satisfies Approval;
const calls=vi.hoisted(()=>({decide:vi.fn<typeof Api.decideApproval>(),join:vi.fn<typeof Access.requestJoin>(),permissions:vi.fn(),read:vi.fn()}));
const originalAppearance=useAppearance.getState();
vi.mock('@/api/approvals',async original=>({...await original<typeof Api>(),decideApproval:(...args:Parameters<typeof Api.decideApproval>)=>calls.decide(...args)}));
vi.mock('@/api/access',async original=>({...await original<typeof Access>(),requestJoin:(...args:Parameters<typeof Access.requestJoin>)=>calls.join(...args),permissionsQuery:{queryKey:['quick-permissions'],queryFn:calls.permissions},accessRequestQuery:(id:string)=>({queryKey:['quick-batch',id],queryFn:calls.read})}));
vi.mock('@/ui/UserChip',()=>({UserChip:({user}:{user:UserRef})=><button data-person-chip={user.id}>{user.display_name}</button>}));
beforeEach(()=>{vi.clearAllMocks();sessionStorage.clear();calls.decide.mockResolvedValue({...approval,state:'approved',decided_at:'2026-10-26T12:01:00Z',decided_by:ada});calls.join.mockResolvedValue({batch_id:'batch',approvals:[approval]});calls.read.mockResolvedValue({batch_id:'batch',approvals:[approval],complete:false});});
afterEach(async()=>{cleanup();useAppearance.setState(originalAppearance);await setLocale('en');});
for(const locale of ['en','zh-CN'] as const)it(`each membership duration submits exactly the configured server-duration choice (${locale})`,async()=>{
 await setLocale(locale);
 for(const duration of approval.quick_grant_durations){
  const settled=vi.fn();const view=show(<AccessCard items={[approval]} settled={new Map()} live onSettled={settled}/>);
  expect(screen.getByText(i18n.t('approvals.quick.join'))).toBeInTheDocument();
  // Duration words use the application's formatter; their order is the configured order.
  const row=screen.getByRole('button',{name:i18n.t('approvals.quick.always')}).parentElement;
  if(row===null)throw new Error('No quick grant row');
  expect(row.querySelectorAll('button')).toHaveLength(6);
  const index=approval.quick_grant_durations.indexOf(duration);
  const button=row.querySelectorAll('button')[index];if(button===undefined)throw new Error('No configured choice');
  await userEvent.click(button);
  await waitFor(()=>expect(calls.decide).toHaveBeenLastCalledWith(approval.id,{approve:true,quick_duration:duration}));
  expect(settled).toHaveBeenCalledOnce();
  view.unmount();
 }
});
it('a late decider receives the stored outcome through the shared card handler',async()=>{
 const current={...approval,state:'denied' as const,decided_at:'2026-10-26T12:01:00Z',decided_by:ada,decision:{approve:false,note:'Not this group'}};
 calls.decide.mockRejectedValue(new ApiError({type:'orch:error:approval.already_decided',request_id:'test-late-decision',code:'approval.already_decided',status:409,title:'Already decided',detail:'Already decided',current},null));
 const settled=vi.fn();show(<AccessCard items={[approval]} settled={new Map()} live onSettled={settled}/>);
 await userEvent.click(screen.getByRole('button',{name:i18n.t('approvals.quick.always')}));
 await waitFor(()=>expect(settled).toHaveBeenCalledWith(current));
});
it('the private group form sends the shared label and a membership request with no permission catalogue read',async()=>{
 show(<AccessRequest missing={[{object:{kind:'chat',id:'group',name:'Shared group',owner:null},permissions:[],options:null,ask:[],ask_more:0,override:false,closed:null}]} fixed privateReference membership sharedLabel="Shared group" objectId="group"/>);
 expect(screen.getByText(i18n.t('access.request.join'))).toBeInTheDocument();
 await userEvent.type(screen.getByRole('textbox',{name:i18n.t('access.request.reason')+' '+i18n.t('ui.field.optional')}),'Working together');
 await userEvent.click(screen.getByRole('button',{name:i18n.t('access.request.send')}));
 await waitFor(()=>expect(calls.join).toHaveBeenCalledWith('group',{reason:'Working together',label:'Shared group'},expect.any(String)));
 expect(calls.permissions).not.toHaveBeenCalled();expect(await screen.findByRole('button',{name:i18n.t('access.request.cancel')})).toBeEnabled();
});
for(const locale of ['en','zh-CN'] as const)it(`a recorded grant names its duration and formats the decision moment (${locale})`,async()=>{
 await setLocale(locale);useAppearance.setState({locale,timeZone:'UTC'});
 const now=Date.now(),at=new Date(now-60_000).toISOString();
 const current={...approval,state:'approved' as const,decided_at:at,decided_by:ada,decision:{approve:true,expires_at:new Date(Date.parse(at)+7*86_400_000).toISOString()}};
 show(<AccessCard items={[current]} settled={new Map()} live onSettled={vi.fn()}/>);
 const status=screen.getByRole('status');
 expect(status).toHaveTextContent(locale==='en'?'for 7 days':'有效期7天');
 const moment=status.querySelector('time');expect(moment).toHaveAttribute('datetime',at);
 expect(moment).toHaveTextContent(formatRelative(at,{locale,timeZone:'UTC'},now));
});
for(const locale of ['en','zh-CN'] as const)it(`the custom date has one visible select label and its own accessible name (${locale})`,async()=>{
 await setLocale(locale);show(<AccessCard items={[approval]} settled={new Map()} live onSettled={vi.fn()}/>);
 await userEvent.click(screen.getByRole('button',{name:i18n.t('approvals.quick.other')}));
 await userEvent.click(screen.getByRole('combobox',{name:i18n.t('approvals.access.ends')}));
 await userEvent.click(screen.getByRole('option',{name:i18n.t('approvals.access.customEnd')}));
 expect(screen.getAllByText(i18n.t('approvals.access.ends'),{exact:true})).toHaveLength(1);
 expect(screen.getByLabelText(i18n.t('approvals.access.endsOn'),{exact:true})).toHaveAttribute('type','datetime-local');
 expect(within(screen.getByRole('combobox',{name:i18n.t('approvals.access.ends')})).getByText(i18n.t('approvals.access.customEnd'))).toBeInTheDocument();
});
