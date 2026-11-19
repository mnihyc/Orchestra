/** A person can pause the request stream, resume it, filter its metadata and open its recorded turn. */
import type { ReactNode } from 'react';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type { UsageEvent } from '@/generated/api-types';
import type { FeedFrame } from '@/realtime/socket';
import { Requests } from '@/features/usage/Requests';
import { OUTCOMES } from '@/features/usage/outcomes';
import { requestRow } from '../doubles/request-row';
import { show } from './support';
import type { TableProps } from '@/ui/Table';
import type * as TableModule from '@/ui/Table';
import { timingFixture } from '../doubles/performance';

const fake = vi.hoisted(() => ({ reads: 0, narrow: false, rows: [] as UsageEvent[], older: [] as UsageEvent[], cursors: [] as string[], receive: undefined as ((frame: FeedFrame) => void) | undefined }));
vi.mock('@/ui/Table', async (importOriginal) => {
  const actual = await importOriginal<typeof TableModule>();
  return { ...actual, Table: (props: TableProps<UsageEvent>) => <actual.Table {...props} columns={fake.narrow ? props.columns.filter((column) => column.id === 'time') : props.columns} /> };
});
vi.mock('@/api/usage', () => ({ fetchEventsPage: (_params: unknown, cursor: string) => { fake.cursors.push(cursor); return Promise.resolve({ items: fake.older, next_cursor: null }); }, eventsQuery: () => ({ queryKey: ['request-log-test'], queryFn: () => { fake.reads += 1; return Promise.resolve({ items: fake.rows, next_cursor: null }); }, initialPageParam: null, getNextPageParam: () => null }) }));
vi.mock('@/api/nodes',()=>({orgsQuery:{queryKey:['orgs'],queryFn:()=>Promise.resolve([])}}));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: (_feed: string, listener: { onFrame: (frame: FeedFrame) => void }) => { fake.receive = listener.onFrame; return { unsubscribe: () => { fake.receive = undefined; } }; } } }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ params, search, children }: { params: { threadId: string }; search: { turn: number }; children: ReactNode }) => <a href={`/thread/${params.threadId}?turn=${search.turn}`}>{children}</a> }));

beforeEach(() => { fake.reads = 0; fake.narrow = false; fake.rows = [requestRow()]; fake.older = []; fake.cursors = []; fake.receive = undefined; });
const span = { from: '2026-10-15', to: '2026-10-15', days: 1, range: 'custom' as const };
const appearance=useAppearance.getState();
const initialTheme=document.documentElement.getAttribute('data-theme');
afterEach(async()=>{useAppearance.setState(appearance);await setLocale('en');if(initialTheme===null)document.documentElement.removeAttribute('data-theme');else document.documentElement.dataset.theme=initialTheme;});

it('locates every outcome in the complete catalogue without loading rows or applying a typed filter', async () => {
  // The selected Running filter must receive an actual running fixture row.
  fake.rows = [requestRow({ running: true })];
  const user = userEvent.setup(), change = vi.fn();
  show(<Requests scope={{ kind: 'me' }} search={{ outcome: 'running' }} span={span} onSearch={change} />);
  await screen.findByText(/Model A/);
  const input = screen.getByRole('combobox', { name: i18n.t('usagePerf.outcome') });
  expect(input).toHaveValue(i18n.t('usagePerf.choices.outcomes.running.name'));
  await user.click(input);
  expect(OUTCOMES.length).toBeGreaterThan(10);
  expect(screen.getAllByRole('option')).toHaveLength(OUTCOMES.length + 1);
  for (const outcome of OUTCOMES) expect(screen.getByRole('option', { name: i18n.t(`usagePerf.choices.outcomes.${outcome}.name`) }))
    .toHaveAccessibleDescription(i18n.t(`usagePerf.choices.outcomes.${outcome}.description`));
  await user.type(input, 'cap_reached');
  expect(change).not.toHaveBeenCalled(); expect(fake.reads).toBe(1); expect(fake.cursors).toEqual([]);
  await user.keyboard('{Enter}');
  expect(change).toHaveBeenCalledExactlyOnceWith({ outcome: 'cap_reached' });
  await user.click(input); await user.type(input, i18n.t('usagePerf.choices.outcomes.all.name'));
  await user.click(screen.getByRole('option', { name: i18n.t('usagePerf.choices.outcomes.all.name') }));
  expect(change).toHaveBeenLastCalledWith({ outcome: undefined });
  expect(fake.reads).toBe(1); expect(fake.cursors).toEqual([]);
});

for(const language of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)it(`keeps request facts separate and translates demo and choice names in ${language}/${theme}`,async()=>{
  await setLocale(language);useAppearance.setState({locale:language,theme});document.documentElement.dataset.theme=theme;fake.narrow=true;
  const named = { harness: { kind: 'fake', name: 'Example agent', names: { 'zh-CN': '示例代理' } } };
  fake.rows=[{ ...requestRow({model:'private_route',model_display:'Mock Large',harness_kind:'fake',effort:'high',tier:'fast',timing:timingFixture({outcome:{state:'told',value:'provider_busy'},error_message:'Please retry this request.',vendor_words:'Please retry this request.'})}), ...named }];
  show(<Requests scope={{kind:'me'}} search={{}} span={span} onSearch={()=>undefined}/>);
  const table=await screen.findByRole('table',{name:i18n.t('usagePerf.requestsView')});
  expect(await within(table).findByText(i18n.t('composer.model.exampleLarge'))).toBeVisible();
  for(const label of ['usage.chip.provider','usage.chip.agent','usage.chip.effort','usage.chip.tier','usagePerf.tokensInOut','usagePerf.requestFirstToken','usagePerf.requestSpeed','usagePerf.wholeTime'] as const)expect(within(table).getByText(i18n.t(label))).toBeVisible();
  expect(within(table).getByText(language === 'zh-CN' ? '示例代理' : 'Example agent')).toBeVisible();
  expect(within(table).getByText(i18n.t('choiceCatalog.effort.high.name'))).toBeVisible();
  expect(within(table).getAllByText('Please retry this request.')).toHaveLength(1);
  expect(table).not.toHaveTextContent(/private_route|Mock Large|mock-large|provider_busy| · |\bfake\b/);
});

for(const language of ['en','zh-CN'] as const)it(`names a third program from a request payload in ${language}`,async()=>{
  await setLocale(language);useAppearance.setState({locale:language});fake.narrow=true;
  const named={harness:{kind:'research-agent',name:'Research assistant',names:{'zh-CN':'研究助理'}}};
  const agent={id:'agent',name:'research-agent',address:'',harness_kind:'research-agent',harness:named.harness,machine:null,disabled:false};
  fake.rows=[{...requestRow({harness_kind:'research-agent',agent}),...named}];
  show(<Requests scope={{kind:'me'}} search={{}} span={span} onSearch={()=>undefined}/>);
  const table=await screen.findByRole('table',{name:i18n.t('usagePerf.requestsView')});
  const name=language==='en'?'Research assistant':'研究助理';
  expect(await within(table).findByText(name)).toBeVisible();
  expect(table).not.toHaveTextContent('research-agent');
  await userEvent.click(screen.getByRole('button',{name:i18n.t('usagePerf.filters')}));
  await userEvent.click(screen.getByRole('combobox',{name:i18n.t('usage.chip.agent')}));
  expect(screen.getByRole('option',{name})).toBeVisible();
});

for (const language of ['en', 'zh-CN'] as const) it(`names an access program while filtering by its original id in ${language}`, async () => {
  await setLocale(language); useAppearance.setState({ locale: language }); fake.narrow = true;
  fake.rows = [requestRow({ access_program: 'daybreakBlue' })];
  const changed = vi.fn();
  show(<Requests scope={{ kind: 'me' }} search={{}} span={span} onSearch={changed} />);
  const name = language === 'en' ? 'Daybreak Blue' : 'Daybreak 蓝';
  const table = await screen.findByRole('table', { name: i18n.t('usagePerf.requestsView') });
  expect(await within(table).findByText(name)).toBeVisible();
  expect(table).not.toHaveTextContent('daybreakBlue');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('usagePerf.filters') }));
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('usage.chip.program') }));
  await userEvent.click(screen.getByRole('option', { name }));
  expect(changed).toHaveBeenCalledWith({ program: 'daybreakBlue' });
});

it('keeps unreported counts distinct from zero in the compact row', async () => {
  fake.narrow = true;
  fake.rows = [requestRow({ running: true, timing: timingFixture({ reported_tokens: null, outcome: { state: 'unread' } }) })];
  show(<Requests scope={{ kind: 'me' }} search={{}} span={span} onSearch={() => undefined} />);
  expect(await screen.findByText(/— in \/ — out/)).toBeInTheDocument();
  expect(screen.queryByText(/0 in \/ 0 out/)).not.toBeInTheDocument();
});

it('names an error in words and preserves the vendor sentence once without a raw outcome code',async()=>{
  fake.rows=[requestRow({timing:timingFixture({outcome:{state:'told',value:'provider_busy'},error_message:'Please retry later',vendor_words:'Please retry later'})})];
  show(<Requests scope={{kind:'me'}} search={{}} span={span} onSearch={()=>undefined}/>);
  const words=await screen.findByText('Please retry later');
  expect(words).toBeVisible();expect(screen.getByText('Rate limited',{exact:true})).toBeVisible();
  expect(screen.queryByText(/provider_busy/)).not.toBeInTheDocument();
  expect(words.textContent.match(/Please retry later/g)).toHaveLength(1);
});

it('names the thread first and omits the person and place already supplied by scope',async()=>{
  const row=requestRow();fake.rows=[row];
  const view=show(<Requests scope={{kind:'node',id:row.project.id}} search={{}} span={span} onSearch={()=>undefined}/>);
  const link=await screen.findByRole('link',{name:'Repair redirects'});
  expect(link).toHaveAttribute('href','/thread/00000000-0000-4000-8000-000000000104?turn=7');
  const table=screen.getByRole('table',{name:'Requests'});
  expect(within(table).getAllByText('Repair redirects')).toHaveLength(1);
  expect(within(table).queryByText(/Web/)).not.toBeInTheDocument();
  view.unmount();
  show(<Requests scope={{kind:'me'}} search={{}} span={span} onSearch={()=>undefined}/>);
  await screen.findByRole('link',{name:'Repair redirects'});
  expect(within(screen.getByRole('table',{name:'Requests'})).queryByText(/Dana/)).not.toBeInTheDocument();
});

it('keeps its place while paused and resumes queued rows, with the actual numbered turn link', async () => {
  const user = userEvent.setup();
  show(<Requests scope={{ kind: 'me' }} search={{}} span={span} onSearch={() => undefined} />);
  await screen.findByText(/Model A/);
  const old = screen.getByRole('link');
  expect(old).toHaveAttribute('href', '/thread/00000000-0000-4000-8000-000000000104?turn=7');
  await user.click(screen.getByRole('button', { name: 'Pause' }));
  act(() => fake.receive?.({ op: 'event', id: 1, feed: 'usage:me', data: { items: [requestRow({ id: 'new', at: '2026-10-15T04:01:00.000Z', model: 'model-b', model_display: 'Model B' })], next_cursor: null } }));
  // Live deliveries are serialized asynchronously; check pause after this arrival was processed.
  expect(await screen.findByRole('button', { name: '1 new' })).toBeInTheDocument();
  expect(screen.queryByText(/Model B/)).not.toBeInTheDocument();
  expect(screen.getByRole('link')).toBe(old);
  await user.click(screen.getByRole('button', { name: 'Resume' }));
  await screen.findByText(/Model B/);
  const table = screen.getByRole('table', { name: 'Requests' });
  expect(within(table).getAllByRole('row')[1]).toHaveTextContent('Model B');
});

it('offers named programme and outcome filters and clears content when the server closes access', async () => {
  const user = userEvent.setup();
  const onSearch = vi.fn();
  show(<Requests scope={{ kind: 'me' }} search={{}} span={span} onSearch={onSearch} />);
  await screen.findByText(/Model A/);
  await user.click(screen.getByText('Filter requests'));
  await user.click(screen.getByRole('combobox', { name: 'Access programme' }));
  await user.click(screen.getByRole('option', { name: 'research' }));
  expect(onSearch).toHaveBeenCalledWith({ program: 'research' });
  await user.click(screen.getByRole('combobox', { name: 'Outcome' }));
  await user.click(screen.getByRole('option', { name: 'Running' }));
  expect(onSearch).toHaveBeenCalledWith({ outcome: 'running' });
  act(() => fake.receive?.({ op: 'closed', id: 1, feed: 'usage:me', reason: 'revoked' }));
  await waitFor(() => expect(screen.queryByText(/Model A/)).not.toBeInTheDocument());
});

it('recovers requests between bounded snapshots by cursor while keeping a paused reader in place', async () => {
  const user=userEvent.setup();
  show(<Requests scope={{ kind: 'me' }} search={{}} span={span} onSearch={() => undefined} />);
  await screen.findByText(/Model A/);
  await user.click(screen.getByRole('button', { name: 'Pause' }));
  fake.older=[requestRow({id:'middle',at:'2026-10-15T04:01:00.000Z',model_display:'Middle model'}),requestRow()];
  act(() => fake.receive?.({op:'event',id:1,feed:'usage:me',data:{items:[requestRow({id:'new',at:'2026-10-15T04:02:00.000Z',model_display:'Newest model'})],next_cursor:'repair-cursor'}}));
  await screen.findByRole('button', {name:'2 new'});
  expect(fake.cursors).toEqual(['repair-cursor']);
  expect(screen.queryByText('Middle model')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', {name:'Resume'}));
  expect(await screen.findByText('Middle model')).toBeInTheDocument();
  expect(screen.getByText('Newest model')).toBeInTheDocument();
});
