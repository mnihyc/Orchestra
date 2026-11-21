/** Catch-up follows the reader's effective default without queuing work merely to inspect it. */
import {QueryClient} from '@tanstack/react-query';
import {act,cleanup,screen,waitFor} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {afterEach,beforeEach,expect,it,vi} from 'vitest';
import type * as Threads from '@/api/threads';
import {client} from '@/api/client';
import {NewLine,RequestedCatchupCard} from '@/features/timeline/Catchup';
import type {Catchup,Meta} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {threadWriter} from '@/realtime/stores/thread';
import {meta} from '../doubles/fixtures';
import {snapshot} from '../unit/continuation-fixtures';
import {show,skipOverflowFallback} from './support';

skipOverflowFallback();
const server=vi.hoisted(()=>({read:vi.fn<(since:number)=>Promise<Catchup>>()}));
vi.mock('@/api/threads',async(load)=>{
  const actual=await load<typeof Threads>();
  return {...actual,catchupQuery:(...args:Parameters<typeof actual.catchupQuery>)=>({...actual.catchupQuery(...args),queryFn:()=>server.read(args[1])})};
});
let current='';
beforeEach(()=>{vi.clearAllMocks();});
afterEach(async()=>{cleanup();if(current!=='')threadWriter.evict(current);current='';client.session.setState({userId:null,status:'signed_out'});await setLocale('en');});

function world(items=10,personal:Record<string,unknown>={}){
  const rows=snapshot(undefined,items);current=rows.thread.id;
  client.session.setState({userId:rows.thread.owner.id,status:'signed_in'});
  threadWriter.reset(current,rows,false);
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const install=(threshold:number)=>cache.setQueryData<Meta>(['meta'],{...meta(),interface_defaults:{...meta().interface_defaults,catch_up_above_items:threshold}});
  install(20);
  cache.setQueryData(['me-state'],{interface:personal});
  server.read.mockResolvedValue({items,seq:items,since:0,turns_completed:1,turns_failed:0,commands:0,files_changed:0,approvals_pending:0,errors:0,milestones:[],announcements:[]});
  return {cache,install,id:current};
}

it.each(['en','zh-CN'] as const)('uses the saved personal threshold instead of the installation value for an automatic card (%s)',async(locale)=>{
  await setLocale(locale);const w=world(10,{catch_up_above_items:3});
  show(<NewLine threadId={w.id}/>,w.cache);
  expect(await screen.findByRole('region',{name:i18n.t('timeline.catchup.title')})).toBeVisible();
  expect(server.read).toHaveBeenCalledExactlyOnceWith(0);
});

it('updates the effective default and source without fetching catch-up when its facts are opened',async()=>{
  const w=world();show(<NewLine threadId={w.id}/>,w.cache);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
  expect(await screen.findByText(/20/)).toBeVisible();
  expect(server.read).not.toHaveBeenCalled();
  act(()=>{w.install(4);});
  await waitFor(()=>expect(server.read).toHaveBeenCalledExactlyOnceWith(0));
  act(()=>{w.cache.setQueryData(['me-state'],{interface:{catch_up_above_items:30}});});
  expect(screen.queryByRole('region',{name:i18n.t('timeline.catchup.title')})).not.toBeInTheDocument();
  expect(await screen.findByText(/30/)).toBeVisible();
});

it('waits for the reader preferences instead of borrowing a default while those preferences are unreported',async()=>{
  const w=world(50);w.cache.removeQueries({queryKey:['me-state'],exact:true});
  show(<NewLine threadId={w.id}/>,w.cache);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.details')}));
  expect(await screen.findByText(i18n.t('factDisclosure.notReported'))).toBeVisible();
  expect(server.read).not.toHaveBeenCalled();
});

it('keeps explicit catch-up available below a chosen automatic threshold',async()=>{
  const w=world(1,{catch_up_above_items:30});
  show(<RequestedCatchupCard threadId={w.id}/>,w.cache);
  expect(await screen.findByRole('region',{name:i18n.t('timeline.catchup.title')})).toBeVisible();
  expect(server.read).toHaveBeenCalledExactlyOnceWith(0);
});
