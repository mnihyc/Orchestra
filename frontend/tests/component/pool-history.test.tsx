/** Choice inspection reads recorded pages and clears private rows when the reader changes. */
import { queryOptions } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { PoolHistory } from '@/features/resources/pools/History';
import type { PoolChoiceDetailsReply, PoolChoicesQuery, PoolChoicesReply, PoolPolicy } from '@/generated/api-types';
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

it.each<[PoolPolicy, boolean]>([
  [{ type: 'round_robin' }, false],
  [{ type: 'least_loaded', measure: 'cpu' }, false],
  [{ type: 'least_cost' }, true],
])('explains a missing estimate only for a price comparison among eligible members (%j)', async (policy, expectsUnknown) => {
  calls.history.mockReturnValue({ ...selected, items: [{ ...selected.items[0]!, policy }] });
  const member = (ordinal: number, label: string, cost: string | null, reason: string | null) => ({
    ordinal, label, cost, reason, resource: { kind: 'host' as const, id: label },
  });
  calls.details.mockResolvedValue({
    id: 'choice', expired_at: null, next: null,
    items: [
      member(1, 'No quote recorded', null, null),
      member(2, 'Recorded zero', '0', null),
      member(3, 'Rejected options', null, 'candidate_parameters'),
    ],
  });
  show(<PoolHistory id="pool" />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('pools.choiceInspect', { name: 'Private host' }) }));
  const list = (await screen.findByText('No quote recorded')).closest('ul')!;
  const [unpriced, priced, skipped] = within(list).getAllByRole('listitem');
  const unknown = i18n.t('pools.estimateUnknown');
  expect(within(unpriced!).queryByText(unknown) !== null).toBe(expectsUnknown);
  expect(within(priced!).getByText(i18n.t('pools.choiceCost', { amount: '0' }))).toBeVisible();
  expect(within(skipped!).queryByText(unknown)).not.toBeInTheDocument();
  expect(within(skipped!).getByText(i18n.t('calls.details.candidate_parameters'))).toBeVisible();
});

it('shows the recorded rate source and cache assumption only on the readable quote', async () => {
  calls.history.mockReturnValue(selected);
  const basis = { source: 'rate_card' as const, uncached_input: true };
  const quote = { ordinal: 1, resource: { kind: 'provider' as const, id: 'p' }, label: 'Priced member', reason: null, cost: '0', cost_basis: basis };
  calls.details.mockResolvedValue({ id: 'choice', expired_at: null, next: null, items: [
    quote,
    { ...quote, ordinal: 2, resource: null, label: null, reason: 'member_hidden' },
    { ordinal: 3, resource: quote.resource, label: 'Other estimate', reason: null, cost: '1' },
  ] });
  show(<PoolHistory id="pool" />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('pools.choiceInspect', { name: 'Private host' }) }));
  const member = (await screen.findByText('Priced member')).closest('li')!;
  for (const label of [i18n.t('usage.records.fromCard'), i18n.t('usageRequest.uncached')]) {
    expect(within(member).getByText(label)).toBeVisible();
    expect(screen.getAllByText(label)).toHaveLength(1);
  }
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
