/** Source approvals share the native plan/run decisions, current authority gates and the settled status line. */
import {cleanup,screen} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import {SourceCard,type SourceApproval} from '@/features/approvals/Source';
import type {UserRef} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {show} from './support';
import {harnessApproval,ALLOW_DENY} from '../doubles/thread-rows';
import {ADA} from '../unit/general-access-fixtures';
const decide=vi.hoisted(()=>vi.fn());
vi.mock('@/features/approvals/Decision',()=>({useDecision:()=>({decide,pending:false,pressed:null,error:null,lost:null})}));
vi.mock('@/realtime/stores/tree',()=>({useNodePath:()=>({path:[]})}));
vi.mock('@/ui/UserChip',()=>({UserChip:({user}:{user:UserRef})=><button data-person-chip={user.id}>{user.display_name}</button>}));
const base=harnessApproval('source-request',{threadId:'thread',itemId:'source-item',title:'Work waits',requester:ADA,request:{request_kind:'command',detail:{command:'review',cwd:'/work',reason:null},options:ALLOW_DENY}});
function source(kind:'plan'|'input'):SourceApproval {
 return kind==='plan'?{...base,kind,request:{item_id:'plan-item',plan:{explanation:'Review the work',steps:[{text:'Check the change',status:'pending'}]}}}:{...base,kind,request:{input_id:'held-input',revision:4,content:[{type:'text',text:'The original authored words'}]}};
}
beforeEach(()=>decide.mockReset());afterEach(async()=>{cleanup();await setLocale('en');});
for(const locale of ['en','zh-CN'] as const){
 it(`a proposed plan uses the canonical Proceed decision (${locale})`,async()=>{
  await setLocale(locale);show(<SourceCard approval={source('plan')} live onSettled={vi.fn()}/>);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('plan.proceed')}));
  expect(decide).toHaveBeenCalledExactlyOnceWith({option_id:'proceed'},'proceed');
  expect(screen.getByText('Review the work')).toBeInTheDocument();
 });
 it(`a held message sends Run it once under the same decision helper (${locale})`,async()=>{
  await setLocale(locale);show(<SourceCard approval={source('input')} live onSettled={vi.fn()}/>);
  expect(screen.getByText('The original authored words')).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('composer.queue.runIt')}));
  expect(decide).toHaveBeenCalledExactlyOnceWith({run:true},'run');
 });
 it(`Keep planning history names its actual person and decision moment (${locale})`,async()=>{
  await setLocale(locale);const row={...source('plan'),state:'answered' as const,decided_by:ADA,decided_at:'2026-10-26T12:01:00Z',decision:{option_id:'keep_planning',note:'Review rollback'}};
  const view=show(<SourceCard approval={row} live onSettled={vi.fn()}/>);
  const status=screen.getByRole('status');expect(status).toHaveTextContent(i18n.t('approvals.source.keep_planning'));
  expect(status.querySelector('time')).toHaveAttribute('datetime',row.decided_at);
  const person=view.container.querySelector(`[data-person-chip="${ADA.id}"]`);expect(person).toBeInTheDocument();expect(person?.closest('[aria-expanded]')).toBeNull();
  expect(screen.queryByRole('button',{name:i18n.t('plan.proceed')})).toBeNull();
 });
}
for(const [canDecide,live] of [[false,true],[true,false],[false,false]] as const)it(`an unavailable decision stays disabled (authority ${canDecide}, live ${live})`,()=>{
 show(<SourceCard approval={{...source('input'),can_decide:canDecide}} live={live} onSettled={vi.fn()}/>);
 expect(screen.getByRole('button',{name:i18n.t('composer.queue.runIt')})).toBeDisabled();expect(decide).not.toHaveBeenCalled();
 expect(screen.getByText(i18n.t(!live?'approvals.waitForLine':'approvals.cannotAnswer'))).toBeInTheDocument();
});
