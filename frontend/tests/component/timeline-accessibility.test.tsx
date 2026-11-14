/** Tool sections and context links remain usable in either catalogue and theme. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { RawCall } from '@/features/timeline/RawCall';
import { McpDisplay } from '@/features/timeline/tools/Mcp';
import { FailureAlert } from '@/features/timeline/FailureAlert';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { effortDisplayName } from '@/features/models/names';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { marker, toolCall, turn } from '../doubles/thread-rows';
import { show } from './support';

const given = vi.hoisted(() => vi.fn<(thread: string, selection: { turn?: string; record?: string }) => void>());
vi.mock('@/features/inspector/givenFocus', () => ({ openGiven: given }));
vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const THREAD = 'accessibility-thread';
const context: TimelineContext = { threadId: THREAD, threadName: 'Login', detail: 'normal', layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const stored = (state: 'completed' | 'running' = 'completed') => turn('turn-1', { ordinal: 1, state, inputId: 'input-1', completedAt: state === 'completed' ? '2026-10-01T10:00:01Z' : null });
const pointer = (id: string, record: string) => marker(id, { marker: 'orch_context', data: { model_input_id: record, purpose: 'input' } }, { turn: 'turn-1' });
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); given.mockClear(); threadWriter.evict(THREAD); });
afterEach(async () => { threadWriter.evict(THREAD); await setLocale(locale);
  if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('timeline in %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it('keeps native runtime names and diagnostics as inert text',()=>{
    const native='<img src=x onerror="globalThis.nativeNoticeExecuted=true">';
    const view=show(<FailureAlert error={{code:'mcp_server_failed',message:'Native wrapper',retryable:false,details:{server:'Source catalog',message:native}}}/>);
    expect(screen.getByRole('alert')).toHaveTextContent(native);
    expect(document.querySelector('img')).toBeNull();
    const item=marker('untrusted-runtime-name',{marker:'harness_changed',data:{program:'Codex',change:'model',subject:null,previous:{state:'unsupported'},value:native,expected:{state:'unsupported'},changed:true,diverged:false,next_turn:true,turn_id:null,reported_at:'2026-10-15T00:00:00.000Z'}});
    if(item.kind!=='marker')throw new Error('A native change is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={item}/></TimelineContextValue.Provider>);
    expect(document.body).toHaveTextContent(native);
    expect(document.querySelector('img')).toBeNull();
    expect(document.querySelector('script')).toBeNull();
  });

  it('keeps native notices readable without printing opaque fields or native subtype codes',()=>{
    const data={program:'Claude Code',change:'system_event',subject:null,previous:{state:'unsupported' as const},value:{kind:'notification',title:'Local index',message:'The index is ready',config:{authorization:'not-for-display'},input:'private-input'},expected:{state:'unsupported' as const},changed:false,diverged:false,next_turn:false,turn_id:null,reported_at:'2026-10-15T00:00:00.000Z'};
    const item=marker('claude-notice',{marker:'harness_changed',data});
    if(item.kind!=='marker')throw new Error('A native notice is a marker');
    const view=show(<TimelineContextValue.Provider value={context}><MarkerLine item={item}/></TimelineContextValue.Provider>);
    expect(document.body).toHaveTextContent(language==='en'?'Notification':'通知');
    expect(document.body).toHaveTextContent('Local index');
    expect(document.body).toHaveTextContent('The index is ready');
    expect(document.body).not.toHaveTextContent('not-for-display');
    expect(document.body).not.toHaveTextContent('private-input');
    expect(document.body).not.toHaveTextContent('system_event');
    const future=marker('future-notice',{marker:'harness_changed',data:{...data,value:{kind:'a_future_native_kind',message:'Native reported words'}}});
    if(future.kind!=='marker')throw new Error('A future native notice is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={future}/></TimelineContextValue.Provider>);
    expect(document.body).toHaveTextContent(language==='en'?'Program event':'程序事件');
    expect(document.body).toHaveTextContent('Native reported words');
    expect(document.body).not.toHaveTextContent('a_future_native_kind');
  });

  it.each(['Codex','Claude Code'])('shows a %s hook run by name without claiming an unreported name',(program)=>{
    const data={program,change:'hook',subject:'Before build',previous:{state:'unsupported' as const},value:{status:'completed'},expected:{state:'unsupported' as const},changed:true,diverged:false,next_turn:false,turn_id:null,reported_at:'2026-10-15T00:00:00.000Z'};
    const named=marker('native-hook',{marker:'harness_changed',data});
    if(named.kind!=='marker')throw new Error('A hook report is a marker');
    const view=show(<TimelineContextValue.Provider value={context}><MarkerLine item={named}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?'Ran hook Before build.':'已运行钩子 Before build。')).toBeVisible();
    const unnamed=marker('unnamed-hook',{marker:'harness_changed',data:{...data,subject:null}});
    if(unnamed.kind!=='marker')throw new Error('A hook report is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={unnamed}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?'A hook ran.':'已运行一个钩子。')).toBeVisible();
    expect(document.body).not.toHaveTextContent('harness_changed');
    const unknown=marker('unknown-hook',{marker:'harness_changed',data:{...data,value:{status:'unknown',stdout:'private-hook-output'}}});
    if(unknown.kind!=='marker')throw new Error('An unknown hook result is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={unknown}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?'Hook Before build reported a result.':'钩子 Before build 报告了结果。')).toBeVisible();
    expect(document.body).not.toHaveTextContent('private-hook-output');
  });

  it.each(['approved','denied'] as const)('keeps an automatic %s verdict as a named factual card',async(outcome)=>{
    const item=toolCall(`native-review-${language}-${look}-${outcome}`,{kind:'other',name:'harness/approvalReview',arguments:null,result:{model:'Review model',reported:true,outcome,reason:'Native review reason',action:{command:'build'},usage:null}},{turn:'turn-1',title:'Do not translate this native title',name:'harness/approvalReview',error:outcome==='denied'});
    if(item.kind!=='tool_call')throw new Error('A review is a tool item');
    item.thread_id=THREAD;
    threadWriter.reset(THREAD,{items:[item]},false);
    show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={THREAD} id={item.id}/></TimelineContextValue.Provider>);
    const label=i18n.t('timeline.permissionReview.title');
    const row=screen.getByRole('button',{name:new RegExp(label)});
    expect(row).toHaveTextContent(i18n.t(`timeline.permissionReview.status.${outcome}`));
    expect(row).not.toHaveTextContent('Do not translate this native title');
    if(row.getAttribute('aria-expanded')==='false')await userEvent.click(row);
    expect(screen.getByText('Native review reason')).toBeVisible();
    expect(screen.getByText(i18n.t('timeline.permissionReview.outcome',{model:'Review model',outcome:i18n.t(`timeline.permissionReview.status.${outcome}`)}))).toBeVisible();
    expect(screen.queryByRole('button',{name:/^(Approve|Deny|批准|拒绝)$/})).not.toBeInTheDocument();
  });

  it('shows the requested and reported model in the reroute error item',()=>{
    show(<FailureAlert error={{code:'model_rerouted',message:'Do not parse the native sentence',retryable:false,details:{from_model:'Chosen model',to_model:'Effective model'}}}/>);
    const alert=screen.getByRole('alert');expect(alert).toHaveTextContent('Chosen model');expect(alert).toHaveTextContent('Effective model');
    expect(alert).not.toHaveTextContent('Do not parse');expect(alert).not.toHaveTextContent('model_rerouted');
  });

  it('names a native close and the subsequent same-segment resume',()=>{
    const data={program:'Codex',change:'session_lifecycle',subject:null,previous:{state:'told' as const,value:'opened'},value:'closed',expected:{state:'unsupported' as const},changed:true,diverged:false,next_turn:false,turn_id:null,reported_at:'2026-10-15T00:00:00.000Z'};
    const closed=marker('native-closed',{marker:'harness_changed',data});
    if(closed.kind!=='marker')throw new Error('The closure is a marker');
    const view=show(<TimelineContextValue.Provider value={context}><MarkerLine item={closed}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?'Codex closed the session.':'Codex 关闭了会话。')).toBeVisible();
    const resumed=marker('native-resumed',{marker:'harness_changed',data:{...data,value:'resumed',previous:{state:'told',value:'closed'}}});
    if(resumed.kind!=='marker')throw new Error('The resume is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={resumed}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?'Resumed Codex.':'已恢复 Codex 会话。')).toBeVisible();
  });

  it('shows a named MCP failure with the source words under the translated cause',()=>{
    show(<FailureAlert error={{code:'mcp_server_failed',message:'Do not parse the native wrapper',retryable:false,details:{server:'Source catalog',message:'Credentials expired'}}}/>);
    const alert=screen.getByRole('alert');
    expect(alert).toHaveTextContent('Source catalog');
    expect(within(alert).getByText('Credentials expired')).toBeVisible();
    expect(alert).not.toHaveTextContent('Do not parse the native wrapper');
    expect(alert).not.toHaveTextContent('mcp_server_failed');
  });

  it('shows native compaction and reconnection without inventing an automatic trigger',()=>{
    const data={program:'Codex',change:'context_compaction',subject:null,previous:{state:'unsupported' as const},value:{duration_ms:null},expected:{state:'unsupported' as const},changed:true,diverged:false,next_turn:false,turn_id:null,reported_at:'2026-10-15T00:00:00.000Z'};
    const compact=marker('native-idle-compaction',{marker:'harness_changed',data});
    if(compact.kind!=='marker')throw new Error('A native compaction is a marker');
    const view=show(<TimelineContextValue.Provider value={context}><MarkerLine item={compact}/></TimelineContextValue.Provider>);
    expect(screen.getByText(i18n.t('timeline.marker.compacted'))).toBeVisible();
    expect(document.body).not.toHaveTextContent(/Automatic|自动|context_compaction|Custom settings|自定义设置/);
    const connection=marker('native-connection',{marker:'harness_changed',data:{...data,change:'environment_connection',subject:'Workspace',value:'connected'}});
    if(connection.kind!=='marker')throw new Error('A native connection is a marker');
    view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={connection}/></TimelineContextValue.Provider>);
    expect(screen.getByText(language==='en'?/environment connection.*connected/:/环境连接.*已连接/)).toBeVisible();
    expect(document.body).not.toHaveTextContent('environment_connection');
  });

  it('shows the program’s actual future sandbox policy with translated values',()=>{
    const item=marker('native-policy',{marker:'harness_changed',data:{program:'Codex',change:'sandbox_mode',subject:null,previous:{state:'told',value:{type:'workspaceWrite'}},value:{type:'readOnly',networkAccess:false},expected:{state:'told',value:{type:'workspaceWrite'}},changed:true,diverged:true,next_turn:true,turn_id:'turn-1',reported_at:'2026-10-15T00:00:00.000Z'}},{turn:'turn-1'});
    if(item.kind!=='marker')throw new Error('The native setting is a marker');
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item}/></TimelineContextValue.Provider>);
    expect(screen.getByText(/Codex/)).toBeVisible();
    expect(screen.getByText(language==='en'?/Read only/:/只读/)).toBeVisible();
    expect(screen.getByText(language==='en'?/Applies to future turns/:/适用于后续轮次/)).toBeVisible();
    expect(document.body).not.toHaveTextContent(/sandbox_mode|readOnly|workspaceWrite/);
  });

  it('omits the first and unchanged model and names only a later model change', () => {
    const first = stored();
    const same = { ...first, id: 'turn-2', ordinal: 2 };
    const changed = { ...first, id: 'turn-3', ordinal: 3, model: 'next-model', model_display: 'Next Model' };
    threadWriter.reset(THREAD, { turns: [first, same, changed] }, false);
    const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={first.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={same.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={changed.id} /></TimelineContextValue.Provider>);
    expect(screen.getByText(new RegExp(changed.model_display))).toBeVisible();
  });

  it('shows the effective model on the first rerouted turn while retaining the requested choice',()=>{
    const requested=stored();
    const actual={...requested,usage_detail:{api_time_ms:null,subagents:null,by_subagent:[],models:[],context_window:null,context_used:null,compact_at:null,shortenings:[],breakdown:false,effective_model:'native-safe',effective_model_display:'Safety model'}};
    threadWriter.reset(THREAD,{turns:[actual]},false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={actual.id}/></TimelineContextValue.Provider>);
    expect(screen.getByText(/Safety model/)).toBeVisible();
    expect(actual.model).toBe(requested.model);
    expect(actual.params).toBe(requested.params);
  });

  it('names an effort change in the catalogue’s words without repeating the unchanged model', () => {
    const first = { ...stored(), effort: 'low' };
    const changed = { ...first, id: 'turn-2', ordinal: 2, effort: 'high' };
    threadWriter.reset(THREAD, { turns: [first, changed] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={changed.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('timeline.footer.effort', { effort: effortDisplayName('high') }))).toBeVisible();
  });

  it('opens all model inputs of a turn through one footer entry, including a repeated marker', async () => {
    const items = [pointer('context-1', 'record-1'), pointer('context-repeat', 'record-1'), pointer('context-2', 'record-2')]
      .map((item, index) => ({ ...item, thread_id: THREAD, seq_created: index + 1, seq_updated: index + 1 }));
    threadWriter.reset(THREAD, { items, turns: [stored()] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId="turn-1" /></TimelineContextValue.Provider>);
    const footer = document.querySelector('[data-footer="turn-1"]');
    if (!(footer instanceof HTMLElement)) throw new Error('The turn has its footer');
    await userEvent.click(within(footer).getByRole('button', { name: i18n.t('thread.turnActions.more') }));
    const links = screen.getAllByRole('menuitem', { name: i18n.t('threadViews.given.context') });
    expect(links).toHaveLength(1);
    await userEvent.click(links[0]!);
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { turn: 'turn-1' });
  });

  it('keeps the exact model-input target when the turn has one recorded input', async () => {
    threadWriter.reset(THREAD, { items: [{ ...pointer('context-only', 'record-exact'), seq_created: 1, seq_updated: 1 }], turns: [stored()] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId="turn-1" /></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.turnActions.more') }));
    await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('threadViews.given.context') }));
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { record: 'record-exact' });
  });

  it('keeps a context recorded before the first turn to one compact link to that record', async () => {
    const item = { ...pointer('session-context', 'session-record'), turn_id: null };
    if (item.kind !== 'marker') throw new Error('The context is a history marker');
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
    expect(screen.getAllByRole('button', { name: i18n.t('threadViews.given.context') })).toHaveLength(1);
    expect(screen.queryByText(i18n.t('threadViews.given.received'), { exact: true })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('threadViews.given.context') }));
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { record: 'session-record' });
  });

  it('uses level-two tool detail sections beneath the thread’s heading', () => {
    const item = toolCall('mcp-1', { kind: 'mcp', server: 'Issues', tool: 'lookup', arguments: { query: 'login' }, result: { found: true }, error: null }, { title: 'Look up login', turn: 'turn-1' });
    if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    show(<><h1>Login</h1><RawCall item={item} /><McpDisplay item={item} /></>);
    const sections = screen.getAllByRole('heading').filter((heading) => heading.tagName !== 'H1');
    expect(sections).toHaveLength(4);
    for (const section of sections) expect(section.tagName).toBe('H2');
    expect(screen.getAllByRole('heading', { name: i18n.t('timeline.mcp.arguments'), level: 2 }).length).toBeGreaterThan(0);
  });

  it.each([null, 'original_author_unavailable', 'unrecognized-private-error'])('keeps the resource resume actor and factual outcome visible for %s', (reason) => {
    const by = { id: 'ada', kind: 'human' as const, display_name: 'Ada Lovelace', username: 'ada', avatar: { text: 'A', color: 1 } };
    const item = marker('resource-enabled', { marker: 'resource_resumed', data: {
      object: { kind: 'provider', id: 'provider-kept' }, name: 'Research API', model: null, by,
      turn_id: 'stopped-turn', reference: 'enable-operation', input_id: reason === null ? 'visible-continue' : null, reason,
    } }, { turn: 'stopped-turn' });
    if (item.kind !== 'marker') throw new Error('The fixture is a history marker');
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
    expect(screen.getByText('Research API')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Ada Lovelace' })).toBeVisible();
    expect(screen.getByText(new RegExp(i18n.t(reason === null ? 'storage.resumed.continuedTask' : 'storage.resumed.stoppedTask')))).toBeVisible();
    if (reason === 'original_author_unavailable') expect(screen.getByText(new RegExp(i18n.t('storage.resumed.reasons.original_author_unavailable')))).toBeVisible();
    expect(screen.queryByText('unrecognized-private-error')).not.toBeInTheDocument();
    expect(screen.queryByText('visible-continue')).not.toBeInTheDocument();
    expect(screen.queryByText('enable-operation')).not.toBeInTheDocument();
    expect(screen.queryByText('stopped-turn')).not.toBeInTheDocument();
  });

});
