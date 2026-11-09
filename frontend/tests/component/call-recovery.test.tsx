/** Recovery rows preserve drafts and show the same pending, stopped and permission states in both languages. */
import { fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CallsRules, CallsSummary } from '@/features/settings/CallsRules';
import { CallRecoveryControls, CallRecoveryStatus } from '@/features/thread/CallRecovery';
import type { CallRecovery, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
import { formatSpan } from '@/lib/time';
import { useAppearance } from '@/lib/appearance';
import { markerSentence } from '@/features/timeline/items/Marker';
import { recoveryCause } from '@/features/calls/History';

vi.mock('@/features/settings/ProviderChoice', () => ({ ProviderDefaultChoice: ({ label }: { label: string }) => <span>{label}</span> }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), providerModelsQuery: (id: string) => ({ queryKey: ['test-models', id], queryFn: () => Promise.resolve([]) }), providerQuery: (id:string)=>({queryKey:['test-provider',id],queryFn:()=>Promise.resolve({id,kind:'api',protocols:['openai-responses'],agent:null,agent_id:null,harness_kind:null})}), agentsQuery:()=>({queryKey:['test-agents'],queryFn:()=>Promise.resolve([])}) }));
vi.mock('@/features/change/target', () => ({ openChange: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const appearance = useAppearance.getState();
afterEach(async () => { vi.useRealTimers(); useAppearance.setState(appearance); await setLocale('en'); });

const rule = { id: 'busy', when: { kind: 'provider_busy' }, steps: [{ type: 'retry', delay_secs: 60, times: 3, backoff: 2 }, { type: 'wait', until: 'person' }] };
const recovery = (): CallRecovery => ({ state: 'scheduled', failure: { code: 'provider_busy', message: 'Synthetic capacity', retryable: true, window: null, said: null, resets_at: null, incomplete: false }, rule_id: 'busy', step: { type: 'retry', delay_secs: 60, times: 3, backoff: 2 }, attempt: 2, due_at: new Date(Date.now() + 42_000).toISOString(), detail: null, count: null, window_secs: null });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows a lost session waiting for its machine and retains its causal history (${locale})`, async () => {
    await setLocale(locale);
    const held: CallRecovery = { ...recovery(), failure: { ...recovery().failure, code: 'session_lost' }, step: { type: 'continue', times: 1 }, attempt: 1, continuing: true, due_at: null, detail: 'machine_offline' };
    const view = show(<CallRecoveryStatus recovery={held} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.details.machine_offline'));
    expect(view.container).toHaveTextContent(i18n.t('calls.attempt', { attempt: '1', total: '1' }));
    expect(view.container).not.toHaveTextContent(i18n.t('calls.needsAttention', { kind: i18n.t('calls.kinds.session_lost') }));
    view.rerender(<CallRecoveryStatus recovery={{ ...held, state: 'running', detail: null }} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.continuing', { kind: recoveryCause('session_lost', i18n.t) }));
    expect(markerSentence({ marker: 'call_recovery', data: { action: 'continue', rule: 'session-lost', kind: 'session_lost', attempt: 1, total: 1, automatic: true, input_id: 'input' } }, i18n.t, { locale })).toContain(recoveryCause('session_lost', i18n.t));
    view.unmount();
    show(<CallsRules shape={{ type: 'json' }} value={[rule]} label={i18n.t('calls.rules')} onChange={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: /→/ }));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('calls.when') }));
    expect(screen.getByRole('option', { name: i18n.t('calls.kinds.session_lost') })).toBeVisible();
  });

  it(`shows administrative pauses without a vendor failure or an attempt count (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<CallRecoveryStatus recovery={{ ...recovery(), state: 'waiting', due_at: null, detail: 'provider_switched_off' }} service />);
    expect(view.container).toHaveTextContent(i18n.t('calls.details.provider_switched_off'));
    expect(view.container).not.toHaveTextContent(i18n.t('calls.kinds.provider_busy'));
    expect(view.container).not.toHaveTextContent(i18n.t('calls.attempt', { attempt: '2', total: '3' }));
    expect(view.container.querySelector('.text-warning-text')).toBeNull();
    view.rerender(<CallRecoveryStatus recovery={{ ...recovery(), state: 'waiting', due_at: null, detail: 'model_switched_off' }} service />);
    expect(view.container).toHaveTextContent(i18n.t('calls.details.model_switched_off'));
  });

  it(`keeps rule rows compact and retains an unfinished editor across Escape (${locale})`, async () => {
    await setLocale(locale);
    const change = vi.fn(); const edited = vi.fn();
    show(<CallsRules shape={{ type: 'json' }} value={[rule]} label={i18n.t('calls.rules')} onChange={change} onEdited={edited} />);
    const row = screen.getByRole('button', { name: /→/ });
    expect(row).toHaveTextContent(i18n.t('calls.kinds.provider_busy'));
    await userEvent.click(row);
    const dialog = screen.getByRole('dialog', { name: i18n.t('calls.editRule') });
    fireEvent.change(within(dialog).getByLabelText(i18n.t('calls.contains')), { target: { value: 'capacity or overload' } });
    expect(change).not.toHaveBeenCalled();
    expect(edited).toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    await userEvent.click(row);
    expect(screen.getByLabelText(i18n.t('calls.contains'))).toHaveValue('capacity or overload');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.save') }));
    expect(change).toHaveBeenCalledOnce();
    expect(change.mock.calls[0]?.[0]).toEqual([{ ...rule, when: { ...rule.when, contains: 'capacity or overload' } }]);
  });

  it(`labels rule matches and shows what empty optional conditions mean (${locale})`, async () => {
    await setLocale(locale);
    show(<CallsRules shape={{ type: 'json' }} value={[rule]} label={i18n.t('calls.rules')} onChange={vi.fn()} />);
    await userEvent.click(screen.getByRole('button', { name: /→/ }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('calls.editRule') });
    expect(within(dialog).getByText(i18n.t('calls.reason'), { selector: 'label' })).toBeVisible();
    expect(within(dialog).getByText(i18n.t('calls.provider'))).toBeVisible();
    for (const key of ['anyModel', 'anyStatus', 'anyWords', 'anyPattern'] as const) expect(within(dialog).getByPlaceholderText(i18n.t(`calls.${key}`))).toBeVisible();
    expect(within(dialog).getByRole('columnheader', { name: i18n.t('calls.steps') })).toBeVisible();
    const menu = within(dialog).getByRole('button', { name: i18n.t('calls.menuOf', { name: i18n.t('calls.stepNumber', { step: '1' }) }) });
    expect(menu).toHaveAttribute('title', i18n.t('calls.menuOf', { name: i18n.t('calls.stepNumber', { step: '1' }) }));
  });

  it(`uses a causal history sentence with the recorded total and an unknown denominator for old history (${locale})`, async () => {
    await setLocale(locale);
    const data = { action: 'continue', rule: 'policy', kind: 'model_refused', attempt: 2, total: 7, automatic: true, input_id: 'input' };
    const sentence = markerSentence({ marker: 'call_recovery', data }, i18n.t, { locale });
    expect(sentence).toBe(locale === 'en' ? 'Continued automatically: the provider refused the request · attempt 2 of 7 · Rule: policy' : '已自动继续：提供方拒绝了请求 · 第2次，共7次 · 规则：policy');
    const { total: _total, ...old } = data;
    const legacy = markerSentence({ marker: 'call_recovery', data: old }, i18n.t, { locale });
    expect(legacy).toContain(i18n.t('calls.attemptOnly', { attempt: '2' }));
    expect(legacy).not.toContain('7');
  });

  it(`keeps an unfinished candidate parameter draft with its step when reordered (${locale})`,async()=>{
    await setLocale(locale);
    const change=vi.fn();
    const held={id:'fallback',when:{kind:'provider_busy'},steps:[{type:'failover',candidate:{provider_id:'provider',model:'One',params:{mode:'one'},dimensions:null}},{type:'failover',candidate:{provider_id:'provider',model:'Two',params:{mode:'two'}}}]};
    show(<CallsRules shape={{type:'json'}} value={[held]} label={i18n.t('calls.rules')} onChange={change} />);
    await userEvent.click(screen.getByRole('button',{name:/→/}));
    let fields=screen.getAllByLabelText(i18n.t('calls.parameters'));
    const first=fields[0],second=fields[1];
    if(first===undefined||second===undefined) throw new Error('two configured candidates must be editable');
    fireEvent.change(first,{target:{value:'{"mode":'}});
    const row=second.closest('tr');
    if(row===null) throw new Error('the candidate must use the shared table row');
    await userEvent.click(within(row).getByRole('button',{name:new RegExp(i18n.t('calls.stepNumber',{step:'2'}))}));
    await userEvent.click(screen.getByRole('menuitem',{name:i18n.t('calls.moveUp')}));
    fields=screen.getAllByLabelText(i18n.t('calls.parameters'));
    expect(fields[1]).toHaveValue('{"mode":');
    expect(screen.getByRole('button',{name:i18n.t('common.save')})).toBeDisabled();
    const moved=fields[1];
    if(moved===undefined) throw new Error('the unfinished candidate remains in the second row');
    fireEvent.change(moved,{target:{value:'{"mode":"recovered"}'}});
    await userEvent.click(screen.getByRole('button',{name:i18n.t('common.save')}));
    expect(change).toHaveBeenCalledExactlyOnceWith([{...held,steps:[held.steps[1],{...held.steps[0],candidate:{...held.steps[0]?.candidate,params:{mode:'recovered'}}}]}]);
  });

  it(`shows known retry progress and the configured guard reason (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<CallRecoveryStatus recovery={recovery()} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.kinds.provider_busy'));
    expect(view.container).toHaveTextContent(i18n.t('calls.attempt', { attempt: '2', total: '3' }));
    view.rerender(<CallRecoveryStatus recovery={{ ...recovery(), state: 'stopped', step: null, due_at: null, detail: 'repeat_limit', count: 4, window_secs: 300 }} />);
    expect(view.container).toHaveTextContent('4');
    expect(view.container.querySelector('[data-call-recovery]')).toHaveAttribute('data-call-recovery', 'stopped');
    expect(view.container).not.toHaveTextContent('Synthetic capacity');
  });

  it(`names a visible continuation from retained effects even when its rule says Retry (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<CallRecoveryStatus recovery={{ ...recovery(), state: 'running', continuing: true }} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.continuing', { kind: recoveryCause('provider_busy', i18n.t) }));
    view.rerender(<CallRecoveryStatus recovery={{ ...recovery(), state: 'running', continuing: false, step: { type: 'continue', times: 3 } }} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.retrying', { kind: recoveryCause('provider_busy', i18n.t) }));
    view.rerender(<CallRecoveryStatus recovery={{ ...recovery(), state: 'running', continuing: true }} service />);
    expect(view.container).toHaveTextContent(i18n.t('calls.retrying', { kind: recoveryCause('provider_busy', i18n.t) }));
  });

  it(`shows a failover destination and its known vendor wait together (${locale})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ locale });
    vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-25T10:00:00.000Z'));
    const view = show(<CallRecoveryStatus recovery={{ ...recovery(), due_at: '2026-10-25T10:01:00.000Z', step: { type: 'failover', candidate: { provider_id: 'provider', model: 'Fallback', params: {} } } }} />);
    expect(view.container).toHaveTextContent('Fallback');
    expect(view.container).toHaveTextContent(formatSpan(60_000, { locale }, 'short'));
    expect(view.container.querySelector('.resource-chip')).toBeVisible();
    view.unmount();
  });

  it(`keeps the executed attempt and rule in recovery history (${locale})`, async () => {
    await setLocale(locale);
    const sentence = markerSentence({ marker: 'call_recovery', data: { action: 'continue', rule: 'policy', kind: 'model_refused', attempt: 2, automatic: true, input_id: 'input' } }, i18n.t, { locale });
    expect(sentence).toContain('2');
    expect(sentence).toContain('policy');
    expect(sentence).toContain(i18n.t('calls.actions.continue'));
  });

  it(`prevents a viewer from retrying and offers distinct actions to a driver (${locale})`, async () => {
    await setLocale(locale);
    const action = vi.fn();
    const thread = { id: 'thread', name: 'Work', access: { permissions: [] } } as unknown as Thread;
    const view = show(<CallRecoveryControls thread={thread} recovery={recovery()} pending={false} onAction={action} />);
    const retry = screen.getByRole('button', { name: i18n.t('calls.retryNow') });
    expect(retry).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(retry);
    expect(action).not.toHaveBeenCalled();
    view.rerender(<CallRecoveryControls thread={{ ...thread, access: { ...thread.access, permissions: ['thread.interact'] } }} recovery={recovery()} pending={false} onAction={action} />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('calls.retryNow') }));
    expect(action).toHaveBeenCalledExactlyOnceWith('retry_now');
    view.rerender(<CallRecoveryControls thread={thread} recovery={{ ...recovery(), state: 'stopped' }} pending={false} onAction={action} />);
    expect(screen.getByRole('button', { name: i18n.t('calls.retry') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('button', { name: i18n.t('calls.continue') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('button', { name: i18n.t('calls.switch') })).toHaveAttribute('aria-disabled', 'true');
  });

  it(`explains an unavailable Switch and keeps Stop retrying an ordinary button (${locale})`, async () => {
    await setLocale(locale);
    const thread = { id: 'thread', name: 'Work', access: { permissions: ['thread.interact'] } } as unknown as Thread;
    const view = show(<CallRecoveryControls thread={thread} recovery={recovery()} pending={false} onAction={vi.fn()} />);
    expect(screen.getByRole('button', { name: i18n.t('calls.stop') }).tagName).toBe('BUTTON');
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
    view.rerender(<CallRecoveryControls thread={thread} recovery={{ ...recovery(), state: 'stopped' }} pending={false} onAction={vi.fn()} />);
    const switching = screen.getByRole('button', { name: i18n.t('calls.switch') });
    expect(switching).toHaveAttribute('aria-disabled', 'true');
    await userEvent.hover(switching);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('calls.configureRequired'));
    view.rerender(<CallRecoveryControls thread={{ ...thread, access: { ...thread.access, permissions: ['thread.interact', 'thread.manage'] } }} recovery={{ ...recovery(), state: 'stopped' }} pending={false} onAction={vi.fn()} />);
    expect(screen.getByRole('button', { name: i18n.t('calls.switch') })).not.toHaveAttribute('aria-disabled', 'true');
  });

  it(`reads inherited rule summaries with their ordered actions (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<CallsSummary value={[rule]} />);
    expect(view.container).toHaveTextContent('→');
    expect(view.container).toHaveTextContent(i18n.t('calls.waitPerson'));
    expect(view.container).not.toHaveTextContent('delay_secs');
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`edits an explicit capability route in the same rule and keeps its opaque configuration (${locale})`, async () => {
    await setLocale(locale); useAppearance.setState({locale});
    const change=vi.fn();
    const candidate={provider_id:'provider',model:'',params:{},route:{route:'web_search.service-api',offering:{provider:'provider',offering:'public-search'},config:{fixture:'kept'}}};
    const held={id:'search-fallback',when:{kind:'unavailable',purpose:'web_search'},steps:[{type:'failover',candidate},{type:'wait',until:'person'}]};
    show(<CallsRules shape={{type:'json'}} value={[held]} label={i18n.t('calls.rules')} onChange={change} />);
    await userEvent.click(screen.getByRole('button',{name:/→/}));
    const dialog=screen.getByRole('dialog',{name:i18n.t('calls.editRule')});
    expect(within(dialog).getByRole('combobox',{name:i18n.t('calls.destination')})).toHaveTextContent(i18n.t('calls.capabilityDestination'));
    expect(within(dialog).queryByLabelText(i18n.t('calls.parameters'))).not.toBeInTheDocument();
    expect(dialog).not.toHaveTextContent('fixture');
    await userEvent.click(within(dialog).getByRole('combobox',{name:i18n.t('calls.purpose')}));
    for(const purpose of ['web_search','page_fetch','image_generation'] as const) expect(screen.getByRole('option',{name:i18n.t(`calls.purposes.${purpose}`)})).toBeVisible();
    await userEvent.keyboard('{Escape}');
    fireEvent.change(within(dialog).getByLabelText(i18n.t('calls.offering')),{target:{value:'another-search'}});
    await userEvent.click(within(dialog).getByRole('button',{name:i18n.t('common.save')}));
    expect(change).toHaveBeenCalledExactlyOnceWith([{...held,steps:[{...held.steps[0],candidate:{...candidate,route:{...candidate.route,offering:{provider:'provider',offering:'another-search'}}}},held.steps[1]]}]);
  });
  it(`names a capability failover and the exhausted chain without showing backend configuration (${locale})`, async () => {
    await setLocale(locale);
    const candidate={provider_id:'provider',model:'',params:{},route:{route:'image_generation.delegation',offering:null,config:{private:'never-publish'}}};
    const view=show(<CallRecoveryStatus recovery={{...recovery(),due_at:null,step:{type:'failover',candidate}}} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.capabilityDestination'));
    expect(view.container).not.toHaveTextContent('never-publish');
    view.rerender(<CallRecoveryStatus recovery={{...recovery(),state:'waiting',due_at:null,step:null,detail:'chain_exhausted'}} />);
    expect(view.container).toHaveTextContent(i18n.t('calls.details.chain_exhausted'));
  });
}


// EN: Thread → paused-service recovery → Resume service; the browser path is web-service-pause.spec.ts.
// 中文：线程 → 暂停服务的恢复状态 → 恢复服务；浏览器路径见 web-service-pause.spec.ts。
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows and resumes a paused service without model replay or primary-provider switch controls (${locale})`, async () => {
    await setLocale(locale);
    const action = vi.fn();
    const thread = { id: 'thread', name: 'Work', access: { permissions: ['thread.interact', 'thread.manage'] } } as unknown as Thread;
    const paused: CallRecovery = { ...recovery(), service: 'web_search', state: 'waiting', due_at: null,
      failure: { ...recovery().failure, code: 'unavailable' }, step: { type: 'wait', until: 'person' } };
    const view = show(<><CallRecoveryStatus recovery={paused} /><CallRecoveryControls thread={thread} recovery={paused} pending={false} onAction={action} /></>);
    expect(view.container).toHaveTextContent(i18n.t('calls.servicePausedPerson', { service: i18n.t('capabilityRoutes.names.web_search'), kind: i18n.t('calls.kinds.unavailable') }));
    expect(screen.queryByRole('button', { name: i18n.t('calls.retry') })).toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('calls.continue') })).toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('calls.switch') })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('calls.resumeService') }));
    expect(action).toHaveBeenCalledExactlyOnceWith('retry_now');
    const reset = '2099-11-09T12:00:00Z';
    view.rerender(<CallRecoveryStatus recovery={{ ...paused, state: 'scheduled', due_at: reset, step: { type: 'wait', until: 'reset' } }} />);
    expect(view.container.querySelector('time')).toHaveAttribute('datetime', reset);
    expect(view.container).toHaveTextContent(i18n.t('capabilityRoutes.names.web_search'));
  });
}
