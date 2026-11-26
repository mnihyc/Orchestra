/** Real generated reply shapes feed the shared strip; actions retain reader, native target and root. */
import type * as Summaries from '@/api/summaries';
import type * as Resources from '@/api/resources';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { queryOptions } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type { SummarySnapshot as Reply } from '@/generated/api-types';
import { ResourceSummary } from '@/features/summaries/ResourceSummary';
import { ChatSummaries } from '@/features/summaries/ChatSummaries';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
import { planProvider } from '../unit/provider-plan-fixtures';

const source = vi.hoisted(() => ({ reply: undefined as Reply | undefined, readFailures: 0, reads: vi.fn(), refresh: vi.fn(), ask: vi.fn(), plan:vi.fn() }));
vi.mock('@/api/resources', async (load)=>({...await load<typeof Resources>(),
  providerQuery:(id:string)=>({queryKey:['summary-plan',id],queryFn:source.plan,retry:false}),
}));
vi.mock('@/features/ask/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), openAskWithDraft: source.ask }));
vi.mock('@/api/summaries', async (original) => ({ ...await original<typeof Summaries>(),
  resourceSummaryQuery: (target: { kind: string; id: string }) => queryOptions({ queryKey: ['native-summary-case', target.kind, target.id], queryFn: () => {
    source.reads(target);
    if (source.readFailures > 0) { source.readFailures -= 1; return Promise.reject(new Error('The free readback failed')); }
    return Promise.resolve(source.reply);
  }, staleTime: 0, retry: false }),
  refreshResourceSummary: source.refresh,
}));
const initial = (): Reply => ({ root_org_id: 'root-b', target: { kind: 'document', id: 'doc' }, inspecting: false, revision: 1, state: 'fresh', text: 'A readable source paragraph.',
  producer: { kind: 'model', title: 'Hidden supplier canary', provider: null, offering: null }, updated_at: '2026-11-08T10:00:00Z', route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null });
afterEach(async () => { cleanup(); source.readFailures = 0; source.reads.mockClear(); source.ask.mockClear(); source.refresh.mockReset(); source.plan.mockReset(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps the actual private subscription hidden when the default later changes to a readable provider (${locale})`, async () => {
    await setLocale(locale);
    const provider=planProvider();
    source.plan.mockResolvedValue(provider);
    source.reply={...initial(),program:{helper_id:null,actor:null,provider:null},route:{
      capability:'summary',detail:null,item:'summary',ledger_unit:'tokens',mode:null,modes:[],
      offering:'future-model',prices:{},program_default:false,provider:{id:provider.id,title:provider.name},
      reason:null,route_view:{id:'summarizer-model.complete',kind:'service',orch_tool:null,disable_native:false},
      setting:'auto.summarizer',title:'Summary',
    }};
    show(<ResourceSummary target={{kind:'document',id:'doc'}} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button',{name:i18n.t('summaryBox.details')}));
    expect(await screen.findByText(i18n.t('providerPlan.roomHidden'))).toBeVisible();
    expect(screen.queryByText(i18n.t('timeline.capabilitySubscription'))).not.toBeInTheDocument();
    expect(source.plan).not.toHaveBeenCalled();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`shows a summary's readable subscription facts without disclosing a private helper or starting work (${locale})`, async () => {
    await setLocale(locale);
    const provider = planProvider();
    source.plan.mockResolvedValue(provider);
    source.reply = {...initial(),program:{helper_id:null,actor:null,provider:{id:provider.id,title:provider.name}}};
    const page = show(<ResourceSummary target={{kind:'document',id:'doc'}} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    expect(source.plan).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button',{name:i18n.t('summaryBox.details')}));
    expect(screen.queryByText(i18n.t('summaryBox.noRates'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByText(i18n.t('timeline.capabilitySubscription')));
    expect(await screen.findByRole('meter',{name:'Weekly window (Fable)'})).toHaveAttribute('aria-valuenow','81');
    expect(page.container.querySelector('[data-capability-helper]')).toBeNull();
    expect(source.refresh).not.toHaveBeenCalled();
    expect(source.ask).not.toHaveBeenCalled();
    expect(source.plan).toHaveBeenCalledTimes(1);
  });
  it.each(['thread', 'topic'] as const)(`keeps a subscription-only %s quiet and explains the missing model on opening Info (${locale})`, async (kind) => {
    await setLocale(locale);
    source.reply = { ...initial(), target: { kind, id: kind }, state: 'no_summarizer', producer: null, manual_problem: 'no_model', text: null, updated_at: null };
    const configure = vi.fn();
    show(<ResourceSummary target={{ kind, id: kind }} reference={`/${kind}/${kind}`} onConfigure={configure} />);
    await screen.findByRole('heading', { name: i18n.t('summaryBox.title') });
    expect(screen.queryByText(i18n.t('summaryBox.state.no_summarizer'))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('summaryBox.manualModel'))).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    const info = within(await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') }));
    expect(info.getByText(i18n.t('summaryBox.state.no_summarizer'))).toBeVisible();
    expect(info.getByText(i18n.t('summaryBox.manualModel'))).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled(); expect(configure).not.toHaveBeenCalled();
    await userEvent.keyboard('{Escape}');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.choose') }));
    expect(configure).toHaveBeenCalledOnce();
  });
  it(`keeps a chat with no messages free of catch-up and title controls or summary reads (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), text: null, producer: null, state: 'empty' };
    const view = show(<ChatSummaries id="empty-chat" range={{ after: 0, through: 0 }} named={false} shown hasMessages={false} />);
    await act(async () => { await Promise.resolve(); });
    expect(view.container.querySelector('[data-chat-summaries]')).toBeNull();
    expect(source.reads).not.toHaveBeenCalled();
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled();
    view.rerender(<ChatSummaries id="empty-chat" range={{ after: 0, through: 0 }} named={false} shown hasMessages />);
    await waitFor(() => expect(source.reads).toHaveBeenCalledExactlyOnceWith({ kind: 'chat_title', id: 'empty-chat' }));
    expect(screen.getByText(i18n.t('summaryOptions.fields.chat_title'), { selector: 'summary span' })).toBeVisible();
    expect(screen.queryByRole('heading', { name: i18n.t('summaryBox.catchUp') })).not.toBeInTheDocument();
  });

  it(`keeps an authored chat title unread until its disclosure opens (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), text: null, producer: null, state: 'empty' };
    show(<ChatSummaries id="named-chat" range={{ after: 8, through: 8 }} named shown hasMessages />);
    await act(async () => { await Promise.resolve(); });
    expect(source.reads).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.catchUpActions.ask') })).not.toBeInTheDocument();
    await userEvent.click(screen.getByText(i18n.t('summaryOptions.fields.chat_title'), { selector: 'summary span' }));
    await waitFor(() => expect(source.reads).toHaveBeenCalledExactlyOnceWith({ kind: 'chat_title', id: 'named-chat' }));
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`keeps a discarded attempt distinct from the previous paragraph with its own producer, payer and output facts (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), state: 'stale', last_attempt: {
      outcome: 'discarded', discard_reason: 'source_changed', at: '2026-11-09T10:00:00Z',
      producer: { kind: 'model', title: 'Discarded producer', offering: 'discarded-model', provider: null },
      usage_owner: { id: 'attempt-payer', kind: 'human', username: 'attempt-payer', display_name: 'Recorded payer', avatar: { text: 'P', color: 2 } },
      captured_usage: { input_tokens: 9, output_tokens: 0, total_tokens: 9, cached_input_tokens: null, cache_write_tokens: null, reasoning_tokens: null },
      result_facts: { flattened: true, first_line_only: false, removed_characters: 12, unknown_tags: 0, duplicate_tags: 0, over_budget_tags: 0, rejected_whole: false, captured_usage: null },
    } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    const body = within(await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') }));
    const attempt = within(body.getByLabelText(locale === 'en' ? 'Recorded outcome' : '已记录的结果'));
    expect(attempt.getByText('Discarded producer')).toBeVisible();
    expect(attempt.getByText('Recorded payer')).toBeVisible();
    expect(attempt.getByText(i18n.t('summaryFacts.discard.source_changed'))).toBeVisible();
    expect(attempt.getByText(locale === 'en' ? '12 characters removed' : '已删减 12 个字符')).toBeVisible();
    expect(attempt.getByText(locale === 'en' ? /0 tokens/u : /0 词元/u)).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled();
  });
  it(`shows a pool for the next refresh without attributing one member's rate to it (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), route: null, refresh_proposal: {
      revision: 'pool', producer: { kind: 'pool', title: 'Model pool', provider: null, offering: null },
      usage_owner: null, input_estimate_tokens: null, output_allowance_tokens: null, estimate_basis: 'assumed', problem: null,
    } };
    show(<ResourceSummary target={{kind: 'document', id: 'doc'}} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t('pools.selectedModel'))).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    const details = within(await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') }));
    expect(details.getByText(i18n.t('pools.selectedModel'))).toBeVisible();
    expect(details.queryByText(i18n.t('summaryBox.baseRates'))).not.toBeInTheDocument();
    expect(source.refresh).not.toHaveBeenCalled();
  });

  it(`keeps the prior model and measured outcome separate from the next refresh and its payer (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), producer: { kind: 'model', title: 'Recorded A', offering: 'model-a', provider: null },
      result_facts: { flattened: true, first_line_only: false, removed_characters: 12, unknown_tags: 0, duplicate_tags: 0, over_budget_tags: 0, rejected_whole: false,
        captured_usage: { input_tokens: 9, output_tokens: 0, total_tokens: 9, cached_input_tokens: null, cache_write_tokens: null, reasoning_tokens: null } },
      refresh_proposal: { revision: 'proposal-b', producer: { kind: 'model', title: 'Proposed B', offering: 'model-b', provider: null },
        usage_owner: { id: 'payer', kind: 'human', username: 'payer', display_name: 'Actual payer', avatar: { text: 'P', color: 2 } },
        input_estimate_tokens: 47, output_allowance_tokens: 321, estimate_basis: 'assumed', problem: null } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText('Recorded A')).toBeVisible();
    expect(screen.getByText('Proposed B')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') });
    const last = within(within(dialog).getByLabelText(locale === 'en' ? 'Last result' : '上次结果'));
    const next = within(within(dialog).getByLabelText(locale === 'en' ? 'Next refresh' : '下次刷新'));
    expect(last.getByText('Recorded A')).toBeVisible();
    expect(last.queryByText('Proposed B')).not.toBeInTheDocument();
    expect(next.getByText('Proposed B')).toBeVisible();
    expect(next.queryByText('Recorded A')).not.toBeInTheDocument();
    expect(next.getByText('Actual payer')).toBeVisible();
    expect(last.getByText(locale === 'en' ? '12 characters removed' : '已删减 12 个字符')).toBeVisible();
    expect(last.getByText(locale === 'en' ? /0 tokens/u : /0 词元/u)).toBeVisible();
    expect(next.getByText(locale === 'en' ? /47 tokens/u : /47 词元/u)).toBeVisible();
    expect(next.getByText(locale === 'en' ? /321 tokens/u : /321 词元/u)).toBeVisible();
    expect(screen.getAllByRole('button', { name: i18n.t('summaryBox.details') })).toHaveLength(1);
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled();
  });

  it(`does not fill an older result's missing output facts from the next refresh (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') });
    expect(within(dialog).getAllByText(locale === 'en' ? 'Not reported' : '尚未报告').length).toBeGreaterThan(0);
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it.each(['no_summarizer', 'fresh'] as const)(`distinguishes co-mounted summary subjects in %s (${locale})`, async (state) => {
    await setLocale(locale); source.reply = { ...initial(), state, producer: null, updated_at: null, text: state === 'fresh' ? 'A readable source paragraph.' : null };
    const configure = vi.fn();
    const targets = [
      { kind: 'thread', id: 'thread', reference: '/thread/thread' },
      { kind: 'chat', id: 'chat', reference: '/chat/chat' },
      { kind: 'chat_title', id: 'chat', reference: '/chat/chat-title' },
    ] as const;
    const words = locale === 'en' ? {
      details: ['Summary details', 'Catch-up details', 'Chat title details'],
      ask: ['Ask about this', 'Ask about catch-up', 'Ask about chat title'],
      refresh: ['Refresh summary', 'Refresh catch-up', 'Refresh chat title'],
      choose: ['Choose summarizer', 'Choose catch-up summarizer', 'Choose title summarizer'],
    } : {
      details: ['摘要详情', '补看详情', '聊天标题详情'],
      ask: ['就此提问', '就补看提问', '就聊天标题提问'],
      refresh: ['刷新摘要', '刷新补看摘要', '刷新聊天标题'],
      choose: ['选择摘要器', '选择补看摘要器', '选择标题摘要器'],
    };
    show(<>{targets.map(({ reference, ...target }) => <ResourceSummary key={target.kind} target={target} reference={reference} onConfigure={configure} />)}</>);
    await screen.findAllByRole('button', { name: i18n.t('summaryBox.details') });
    for (const name of [...words.details, ...words.ask, ...(state === 'fresh' ? words.refresh : words.choose)]) {
      expect(screen.getAllByRole('button', { name })).toHaveLength(1);
    }
    const status = i18n.t(`summaryBox.state.${state}`);
    if (state === 'fresh') {
      expect(screen.getAllByText(status, { exact: true })).toHaveLength(1);
      for (const subject of [i18n.t('summaryBox.catchUp'), i18n.t('summaryOptions.fields.chat_title')]) {
        expect(screen.getByText(`${subject} · ${status}`, { exact: true })).toBeVisible();
      }
    } else {
      expect(screen.queryByText(status, { exact: true })).not.toBeInTheDocument();
    }
    for (const name of words.ask) await userEvent.click(screen.getByRole('button', { name }));
    expect(source.ask).toHaveBeenCalledTimes(3);
    for (const [index, { reference }] of targets.entries()) expect(source.ask).toHaveBeenNthCalledWith(index + 1, { root: 'root-b', text: [i18n.t('summaryBox.ask'), reference, ...(state === 'fresh' ? ['> A readable source paragraph.'] : [])].join('\n\n') });
    expect(source.refresh).not.toHaveBeenCalled(); expect(configure).not.toHaveBeenCalled();
  });
  it.each(['usage_measurement', 'plan_reserve'] as const)(`keeps the paragraph while automatic work waits for %s (${locale})`, async (reason) => {
    await setLocale(locale);
    source.reply = { ...initial(), state: 'scheduled', wait: { reason, until: '2026-11-09T02:00:00Z' }, poll_after_ms: 900_000 };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t(`summaryBox.wait.${reason}`))).toBeVisible();
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`shows the actual scheduled time and reason while retaining the paragraph (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), state: 'scheduled', wait: { reason: 'interval', until: '2026-11-09T02:00:00Z' }, poll_after_ms: 900_000 };
    const page = show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t('summaryBox.state.scheduled'))).toBeVisible();
    expect(screen.getByText(i18n.t('summaryBox.wait.interval'))).toBeVisible();
    expect(page.container.querySelector('time[datetime="2026-11-09T02:00:00Z"]')).not.toBeNull();
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toHaveAttribute('aria-disabled', 'true');
  });
  it.each(['interval', 'recovery', 'queue'] as const)(`distinguishes the last successful update from the current wait (${locale}, %s)`, async (reason) => {
    await setLocale(locale);
    const due = reason === 'queue' ? null : '2026-11-16T02:00:00Z';
    source.reply = { ...initial(), state: due === null ? 'updating' : 'scheduled', wait: { reason, until: due } };
    const page = show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    const updated = await screen.findByText(locale === 'en' ? 'Last updated' : '上次更新', { exact: false });
    expect(updated.querySelector('time[datetime="2026-11-08T10:00:00Z"]')).not.toBeNull();
    const waitWords = locale === 'en'
      ? { interval: 'Next update', recovery: 'Waiting to retry', queue: 'Waiting for a worker' }[reason]
      : { interval: '下次更新', recovery: '等待重试', queue: '等待工作器' }[reason];
    expect(screen.getByText(waitWords, { exact: false })).toBeVisible();
    if (due !== null) expect(page.container.querySelector(`time[datetime="${due}"]`)).not.toBeNull();
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`offers a personal default refresh without an automatic producer or configuration action (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), state: 'empty', text: null, producer: null, manual_problem: null };
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), revision: 2, text: 'A personal default summary.' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await screen.findByText('A personal default summary.');
    expect(source.refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.choose') })).not.toBeInTheDocument();
  });
  it.each(['no_model', 'cap_reached'] as const)(`explains %s without a paid attempt (${locale})`, async (problem) => {
    await setLocale(locale); source.reply = { ...initial(), state: 'no_summarizer', text: null, producer: null, manual_problem: problem };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByRole('heading', { name: i18n.t('summaryBox.title') });
    const reason = i18n.t(problem === 'cap_reached' ? 'summaryBox.manualCap' : 'summaryBox.manualModel');
    expect(screen.queryByText(reason)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    expect(within(await screen.findByRole('dialog', { name: i18n.t('summaryBox.details') })).getByText(reason)).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`shows actual stored input coverage without another refresh or Ask (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), input: { included_items: 5, shortened_items: 3, omitted_items: 12 } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    expect(await screen.findByText(i18n.t('summaryBox.inputFacts', { included: 5, shortened: 3, omitted: 12 }))).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled();
  });
  it(`does not invent zero coverage when the stored source supplies no input facts (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    expect(screen.queryByText(i18n.t('summaryBox.inputFacts', { included: 0, shortened: 0, omitted: 0 }))).not.toBeInTheDocument();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`keeps passive inspection readable without a new model or Ask act (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), inspecting: true };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    const ask = screen.getByRole('button', { name: i18n.t('summaryBox.ask') });
    const refresh = screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
    expect(ask).toHaveAttribute('aria-disabled', 'true');
    expect(refresh).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(ask); await userEvent.click(refresh);
    expect(source.ask).not.toHaveBeenCalled(); expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`opens a quoted editable draft in the actual source root without sending (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    expect(screen.getByText(i18n.t('summaryBox.model'))).toBeVisible();
    expect(screen.queryByText('Hidden supplier canary')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(source.ask).toHaveBeenCalledExactlyOnceWith({ root: 'root-b', text: `${i18n.t('summaryBox.ask')}\n\n/doc/doc\n\n> A readable source paragraph.` });
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`refreshes only its admitted target then reads native state (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), revision: 2, state: 'updating' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(1));
    expect(source.refresh.mock.calls[0]?.[0]).toEqual({ kind: 'document', id: 'doc' });
    expect(source.refresh.mock.calls[0]?.[1]).toEqual(expect.any(String));
    await screen.findByText(i18n.t('summaryBox.state.updating'));
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
  });
  it(`says why a refresh failed in that reason's own sentence beside the paragraph it keeps (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), state: 'failed', problem: { type: 'orch:error:provider.unavailable', code: 'provider.unavailable', status: 503, title: '',
      detail: 'The machine that keeps the key of the provider is offline.', request_id: 'summary-read', reason: 'keeper_offline' } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(i18n.t('summaryBox.failure.keeper_offline'));
    expect(alert).toHaveTextContent(i18n.t('summaryBox.failure.kept'));
    expect(alert).not.toHaveTextContent(i18n.t('errors.provider.unavailable'));
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
  });
  it(`gives a refresh that found nothing new as a plain line and no error (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), state: 'failed', text: null, producer: null, updated_at: null, problem: { type: 'orch:error:provider.unavailable', code: 'provider.unavailable', status: 503, title: '',
      detail: 'There is nothing new to summarize.', request_id: 'summary-read', reason: 'nothing_new' } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t('summaryBox.nothingNew.other'))).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('summaryBox.state.failed'))).not.toBeInTheDocument();
  });
  it(`blocks Refresh of a catch-up whose range is empty and says why, under the name of what it covers (${locale})`, async () => {
    await setLocale(locale);
    source.reply = { ...initial(), target: { kind: 'chat', id: 'chat' }, state: 'empty', text: null, producer: null, updated_at: null };
    show(<ResourceSummary target={{ kind: 'chat', id: 'chat' }} reference="/chat/chat" range={{ after: 6, through: 6 }} />);
    expect(await screen.findByRole('heading', { name: i18n.t('summaryBox.catchUp') })).toBeVisible();
    const refresh = screen.getByRole('button', { name: i18n.t('summaryBox.catchUpActions.refresh') });
    expect(refresh).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(refresh);
    await userEvent.hover(refresh);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('summaryBox.nothingNew.chat'));
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`clears removed projected text even when the native cache revision did not change (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), text: null, producer: null, updated_at: null, state: 'stale' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(screen.queryByText('A readable source paragraph.')).not.toBeInTheDocument());
  });
}

for (const locale of ['en', 'zh-CN'] as const) it(`reuses an unresolved paid refresh identity and allocates a new one after acknowledgement (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockRejectedValueOnce(new Error('The accepted refresh acknowledgement was lost'))
    .mockImplementationOnce(() => { source.reply = { ...initial(), revision: 2, text: 'The acknowledged updated paragraph.' }; return Promise.resolve(); })
    .mockResolvedValue(undefined);
  show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  const refresh = () => screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
  await userEvent.click(refresh()); await screen.findByRole('alert');
  await userEvent.click(refresh());
  await screen.findByText('The acknowledged updated paragraph.');
  expect(source.refresh).toHaveBeenCalledTimes(2);
  expect(source.refresh.mock.calls[1]?.[1]).toBe(source.refresh.mock.calls[0]?.[1]);
  await userEvent.click(refresh());
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(3));
  expect(source.refresh.mock.calls[2]?.[1]).not.toBe(source.refresh.mock.calls[0]?.[1]);
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps a pending refresh when the same target is rendered with another property order (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockRejectedValueOnce(new Error('Acknowledgement lost')).mockResolvedValue(undefined);
  const page = show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await screen.findByRole('alert');
  page.rerender(<ResourceSummary target={{ id: 'doc', kind: 'document' }} reference="/doc/doc" />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(2));
  expect(source.refresh.mock.calls[1]?.[1]).toBe(source.refresh.mock.calls[0]?.[1]);
});

for (const locale of ['en', 'zh-CN'] as const) it(`retries only the free read after an acknowledged paid refresh (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockImplementationOnce(() => {
    source.reply = { ...initial(), revision: 2, text: 'The accepted refresh is readable after retry.' };
    source.readFailures = 1;
    return Promise.resolve();
  }).mockResolvedValue(undefined);
  show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await screen.findByRole('alert');
  expect(source.refresh).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await screen.findByText('The accepted refresh is readable after retry.');
  expect(source.refresh).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(2));
  expect(source.refresh.mock.calls[1]?.[1]).not.toBe(source.refresh.mock.calls[0]?.[1]);
});
