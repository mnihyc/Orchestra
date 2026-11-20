/** Choice inspection reads recorded pages and clears private rows when the reader changes. */
import { queryOptions } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { PoolHistory } from '@/features/resources/pools/History';
import type { PoolChoiceDetailsReply, PoolChoicesQuery, PoolChoicesReply } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { show } from './support';

const calls=vi.hoisted(()=>({history:vi.fn<() => PoolChoicesReply>(),details:vi.fn<() => Promise<PoolChoiceDetailsReply>>()}));
vi.mock('@/api/pools',()=>({
  poolChoicesQuery:(id:string,after:PoolChoicesQuery,reader:string)=>queryOptions({queryKey:['choice-history',id,after,reader],queryFn:()=>calls.history()}),
  poolChoiceDetailsQuery:(id:string,choice:string,after:number|null,reader:string)=>queryOptions({queryKey:['choice-details',id,choice,after,reader],queryFn:()=>calls.details()}),
}));
const before=client.session.getState();
afterEach(()=>{client.session.setState(before);vi.clearAllMocks();});
const selected:PoolChoicesReply={items:[{id:'choice',created_at:'2026-11-01T00:00:00.000Z',state:'selected',resource:{kind:'host',id:'host'},label:'Private host',reason:null,policy:{type:'round_robin'},details_expired_at:null}],next:null};

it('opens only the requested recorded detail page and says when it expired',async()=>{
  calls.history.mockReturnValue(selected);
  calls.details.mockResolvedValue({id:'choice',expired_at:'2026-11-02T00:00:00.000Z',items:[],next:null});
  show(<PoolHistory id="pool" />);
  await screen.findByText('Private host');
  expect(calls.details).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('pools.choiceInspect',{name:'Private host'})}));
  expect(await screen.findByText(i18n.t('pools.choiceExpired'))).toBeVisible();
  expect(calls.details).toHaveBeenCalledTimes(1);
});

it('drops the previous reader and its pending private detail response immediately',async()=>{
  client.session.setState({userId:'reader',readerVersion:91});
  calls.history.mockReturnValueOnce(selected).mockReturnValue({items:[{...selected.items[0]!,resource:null,label:null,reason:'member_hidden'}],next:null});
  let complete:((reply:PoolChoiceDetailsReply)=>void)|undefined;
  calls.details.mockReturnValue(new Promise((resolve)=>{complete=resolve;}));
  show(<PoolHistory id="pool" />);
  await screen.findByText('Private host');
  await userEvent.click(screen.getByRole('button',{name:i18n.t('pools.choiceInspect',{name:'Private host'})}));
  await waitFor(()=>expect(calls.details).toHaveBeenCalledTimes(1));
  expect(complete).toBeTypeOf('function');
  act(()=>client.session.setState({readerVersion:92}));
  expect(screen.queryByText('Private host')).not.toBeInTheDocument();
  await act(async()=>{complete?.({id:'choice',expired_at:null,items:[{ordinal:1,resource:{kind:'host',id:'host'},label:'Private detail',reason:null,cost:'123.45'}],next:null});await Promise.resolve();});
  expect(await screen.findByText(i18n.t('pools.hidden'))).toBeVisible();
  expect(screen.queryByText('Private detail')).not.toBeInTheDocument();
  expect(screen.queryByText('123.45')).not.toBeInTheDocument();
});
