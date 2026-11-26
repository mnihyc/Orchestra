/** A session's immutable decisions are separate from current resource versions and expire with its reader. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { SessionSkills } from '@/features/inspector/SessionSkills';
import { ApiError } from '@/api/client';
import { i18n } from '@/i18n';
import { resultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const fixture=vi.hoisted(()=>({calls:0,denied:false,target:null as ThreadReadTarget|null}));
vi.mock('@/api/sessionSkills',()=>({sessionSkillsQuery:(target:ThreadReadTarget,summary:{session_revision:string})=>({
  queryKey:['session-skill-case',summary.session_revision],initialPageParam:null as string|null,retry:false,
  queryFn:({pageParam}:{pageParam:string|null})=>{
    fixture.calls+=1;
    fixture.target=target;
    if(fixture.denied)throw new ApiError({type:'orch:error:authz.denied',code:'authz.denied',title:'Denied',detail:'',status:403,request_id:''},null);
    return Promise.resolve({next_session_changed:true,entries:pageParam===null?{next_cursor:'next',items:[{
      skill:{id:'readable-skill',name:summary.session_revision==='first'?'accepted-v1-skill':'accepted-v2-skill'},contributor:null,source:{kind:'personal'},
      decision:{kind:'selected',version:{id:'recorded-version',number:summary.session_revision==='first'?1:2}},
    }]}:{next_cursor:null,items:[{skill:null,contributor:null,source:{kind:'place',setting:null},decision:{kind:'skipped',reason:'not_usable'}}]}});
  },getNextPageParam:(page:{entries:{next_cursor:string|null}})=>page.entries.next_cursor??undefined,
})}));
beforeEach(()=>{fixture.calls=0;fixture.denied=false;fixture.target=null;});afterEach(()=>cleanup());
function thread(revision='first') {
  const value=snapshot().thread;
  return {...value,binding:value.binding===null?null:{...value.binding,session_skills:{session_revision:revision,segment_id:'segment',applied_at:'2026-11-15T00:00:00Z',supported:true,selected:1,skipped:1}}};
}

it('loads captured versions on intent, searches later pages and replaces the reader with the next accepted session',async()=>{
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const view=show(<SessionSkills thread={thread()}/>,cache);
  try {
    expect(fixture.calls).toBe(0);
    await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
    expect(await screen.findByText('accepted-v1-skill')).toBeVisible();
    expect(screen.getByText('Version 1')).toBeVisible();expect(fixture.calls).toBe(1);
    await userEvent.type(screen.getByRole('searchbox',{name:'Search session skills'}),'Contributor');
    expect(await screen.findByText('Skipped · Contributor lacks Use')).toBeVisible();
    expect(fixture.calls).toBe(2);expect(screen.queryByText('readable-skill')).not.toBeInTheDocument();
    view.rerender(<SessionSkills thread={thread('second')}/>);
    expect(await screen.findByText('accepted-v2-skill')).toBeVisible();
    expect(screen.getByText('Version 2')).toBeVisible();expect(screen.queryByText('accepted-v1-skill')).not.toBeInTheDocument();
  } finally {view.unmount();cache.clear();}
});

it('releases nested names after the current read is refused',async()=>{
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const view=show(<SessionSkills thread={thread()}/>,cache);
  try {
    await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
    await screen.findByText('accepted-v1-skill');fixture.denied=true;
    await act(async()=>{await cache.invalidateQueries({queryKey:['session-skill-case']});});
    await waitFor(()=>expect(screen.queryByText('accepted-v1-skill')).not.toBeInTheDocument());
    expect(screen.queryByText('Version 1')).not.toBeInTheDocument();
  } finally {view.unmount();cache.clear();}
});

it('keeps an old session with no captured receipt unreported without reading current skill resources',async()=>{
  const value=thread();show(<SessionSkills thread={{...value,binding:value.binding===null?null:{...value.binding,session_skills:undefined}}}/>);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
  expect(await screen.findByText('Not reported')).toBeVisible();expect(fixture.calls).toBe(0);
});

it('uses the workspace reader even when the conversation has no open tab',async()=>{
  const value=thread();const read=resultThreadRead(value.id,'workspace-source');
  show(<SessionSkills thread={value} read={read}/>);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
  await screen.findByText('accepted-v1-skill');
  expect(fixture.target).toEqual(read);
});
