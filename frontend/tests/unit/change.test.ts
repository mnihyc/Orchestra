/** What the wizard of changes asks of the server and says of the answer, apart from any page: the request that asks for what was chosen and nothing else, what an error says besides its code, the words of a plan and of what stops it, the way into the wizard and the way into a side thread. */
import { beforeEach, describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { blockersOf, heldBy, planGone, sideLimitOf } from '@/api/forks';
import { changedOptions, changesSomething, initialChoices, NO_OPTIONS, patterns, requestOf, selectionOf } from '@/features/change/choices';
import { askOf, blockerWords, fidelityKey, modelWords, plannedApprovalWords, rangeWords } from '@/features/change/plan';
import { checksOf, committed } from '@/features/change/Progress';
import { openChange, useChangeHost } from '@/features/change/target';
import { openSideAsk, useSideAsk } from '@/features/side-threads/ask';
import type { Job, PlanBlocker, Problem } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

function refusal(code: string, status: number, more: Record<string, unknown> = {}): ApiError {
  const problem = { type: `orch:error:${code}`, code, title: code, status, detail: code, request_id: 'req-1', ...more } as unknown as Problem;
  return new ApiError(problem, null);
}

describe('the request that a choice makes', () => {
  it('asks for what was changed and for nothing the person left alone', () => {
    const choices = { ...initialChoices('replace'), providerId: 'prov-2', agentId: 'agent-2', model: 'gpt-5.4', effort: 'high' };
    expect(requestOf(choices)).toEqual({ result: 'replace', to: { provider_id: 'prov-2', agent_id: 'agent-2', model: 'gpt-5.4', effort: 'high' } });
    // Not a word of the thread's own settings, its page or its name in a switch.
    expect(requestOf({ ...choices, name: 'Ignored', page: 'private' })).toEqual(requestOf(choices));
  });

  it('carries in a switch the history the person chose, and leaves it to the place when they chose none', () => {
    const choices = { ...initialChoices('replace'), providerId: 'prov-2' };
    expect(requestOf(choices)).not.toHaveProperty('history');
    expect(requestOf({ ...choices, history: { mode: 'none', fromTurn: null, toTurn: null } }).history).toEqual({ mode: 'none' });
    expect(requestOf({ ...choices, history: { mode: 'summary', fromTurn: null, toTurn: null } }).history).toEqual({ mode: 'summary' });
    expect(requestOf({ ...choices, history: { mode: 'range', fromTurn: 3, toTurn: null } }).history).toEqual({ mode: 'range', from_turn: 3 });
    // Carry chosen against a place that summarizes is asked for in so many words.
    expect(requestOf({ ...choices, history: { mode: 'all', fromTurn: null, toTurn: null } }).history).toEqual({ mode: 'all' });
  });

  it('is not asked for at all when nothing that is chosen changes where or on what the thread runs', () => {
    const none = initialChoices('replace');
    expect(changesSomething(none)).toBe(false);
    // A model or an effort alone is a setting of the thread, not a switch.
    expect(changesSomething({ ...none, effort: 'low' })).toBe(false);
    expect(changesSomething({ ...none, providerId: 'prov-2' })).toBe(true);
    expect(changesSomething({ ...none, where: { kind: 'use', id: 'env-2', files: { mode: 'all' } } })).toBe(true);
    expect(changesSomething({ ...none, directory: ' sub ' })).toBe(true);
  });

  it('takes the history of a fork up to the turn of the message it was asked from, and nothing of it from before the first', () => {
    expect(requestOf(initialChoices('new', 3))).toEqual({ result: 'new', history: { mode: 'range', to_turn: 3 } });
    expect(requestOf(initialChoices('new', 0))).toEqual({ result: 'new', history: { mode: 'none' } });
    expect(requestOf(initialChoices('new'))).toEqual({ result: 'new' });
    expect(requestOf({ ...initialChoices('new'), history: { mode: 'range', fromTurn: 2, toTurn: 4 } })).toEqual({ result: 'new', history: { mode: 'range', from_turn: 2, to_turn: 4 } });
    expect(requestOf({ ...initialChoices('new'), history: { mode: 'summary', fromTurn: null, toTurn: null } })).toEqual({ result: 'new', history: { mode: 'summary' } });
  });

  it('names a new environment with some of the files by the patterns written, one to a line, without the empty ones', () => {
    const choices = {
      ...initialChoices('new', 2),
      where: { kind: 'new' as const, hostId: null, name: ' Trial ', files: { mode: 'selection' as const, include: 'src/**\n\n README.md \n', exclude: ' dist ' }, home: true, privateViews: false, onlyMe: true },
    };
    expect(requestOf(choices).environment).toEqual({
      new: { host_id: null, name: 'Trial', files: { mode: 'selection', include: ['src/**', 'README.md'], exclude: ['dist'] }, home: true, private_views: false, access: 'only_me' },
    });
    expect(patterns('a\n\n  b  \n')).toEqual(['a', 'b']);
    expect(selectionOf({ mode: 'none' })).toEqual({ mode: 'none' });
    expect(selectionOf({ mode: 'selection', include: 'a', exclude: '' })).toEqual({ mode: 'selection', include: ['a'] });
  });

  it('gives the new thread the page and the name it was asked to have, and the switch of a thread neither', () => {
    const fork = { ...initialChoices('new'), page: 'private' as const, name: ' Experiment ' };
    expect(requestOf(fork, 'topic-1')).toEqual({ result: 'new', name: 'Experiment', topic_id: 'topic-1', access: 'private' });
    expect(requestOf({ ...fork, page: 'place' as const })).toMatchObject({ access: 'defaults' });
    expect(requestOf({ ...fork, page: 'original' as const })).not.toHaveProperty('access');
    expect(requestOf({ ...initialChoices('replace'), page: 'private', name: 'x', providerId: 'prov-2' })).toEqual({ result: 'replace', to: { provider_id: 'prov-2' } });
  });

  it('carries the options that were changed, and counts them for the mark beside Options', () => {
    const options = { ...NO_OPTIONS, activeTurn: 'interrupt' as const, waitMinutes: 3 };
    expect(requestOf({ ...initialChoices('replace'), providerId: 'prov-2', options }).options).toEqual({ active_turn: 'interrupt', wait_minutes: 3 });
    expect(requestOf({ ...initialChoices('replace'), providerId: 'prov-2' })).not.toHaveProperty('options');
    expect(changedOptions(options)).toBe(2);
    expect(changedOptions(NO_OPTIONS)).toBe(0);
  });
});

describe('what a refusal says besides its code', () => {
  it('says that a plan is gone when it is older than fifteen minutes or was used, and not for anything else', () => {
    expect(planGone(refusal('preflight.expired', 409))).toBe(true);
    expect(planGone(refusal('not_found', 404))).toBe(true);
    expect(planGone(refusal('plan.blocked', 409))).toBe(false);
    expect(planGone(new Error('offline'))).toBe(false);
  });

  it('carries the blockers of a plan that cannot be carried out, as the plan would have carried them', () => {
    const blockers = [{ code: 'already_running', message: 'a switch is under way' }, { code: 'access', message: 'no', missing: [] }];
    expect(blockersOf(refusal('plan.blocked', 409, { blockers }))).toEqual(blockers);
    expect(blockersOf(refusal('plan.blocked', 409))).toEqual([]);
    expect(blockersOf(new Error('x'))).toEqual([]);
  });

  it('names the job that holds a thread when a plan was refused for that, and no other', () => {
    expect(heldBy(refusal('conflict.exists', 409, { job_id: 'job-7' }))).toBe('job-7');
    expect(heldBy(refusal('conflict.exists', 409))).toBeNull();
    expect(heldBy(refusal('plan.blocked', 409, { job_id: 'job-7' }))).toBeNull();
  });

  it('says how many side threads are open and how many are allowed when the limit is reached', () => {
    expect(sideLimitOf(refusal('thread.side_limit', 409, { open: 4, max: 4 }))).toEqual({ open: 4, max: 4 });
    expect(sideLimitOf(refusal('thread.side_limit', 409, { open: '4' }))).toBeNull();
    expect(sideLimitOf(refusal('conflict.exists', 409, { open: 4, max: 4 }))).toBeNull();
  });
});

describe('the words of a plan', () => {
  it('names the target approval mode in both languages and never prints wire values', async () => {
    try {
      await setLocale('en');
      expect(plannedApprovalWords({ approval_mode: 'ask' }, i18n.t.bind(i18n))).toBe('Approval: Ask');
      expect(plannedApprovalWords({ approval_mode: 'on_request' }, i18n.t.bind(i18n))).toBe('Approval: On request');
      expect(plannedApprovalWords({}, i18n.t.bind(i18n))).toBeNull();
      expect(plannedApprovalWords({ approval_mode: 'OnRequest' }, i18n.t.bind(i18n))).toBeNull();
      await setLocale('zh-CN');
      expect(plannedApprovalWords({ approval_mode: 'ask' }, i18n.t.bind(i18n))).toBe('审批：询问');
      expect(blockerWords({ code: 'approval_mode_locked', message: 'untranslated' }, null, i18n.t.bind(i18n)).text).toBe(i18n.t('change.failure.approval_mode_locked'));
    } finally {
      await setLocale('en');
    }
  });
  const t = i18n.t.bind(i18n);

  it('uses the same keyed class reason for a move in English and Chinese', async () => {
    const english = 'To take this environment to another host, copy its files first: Kubernetes volumes do not travel with a move.';
    const chinese = '要将此环境迁移到其他主机，请先复制文件：Kubernetes 卷不会随迁移携带。';
    const blocker: PlanBlocker = { code: 'runtime_limit.kubernetes_volume_move', message: english };
    try {
      await setLocale('en');
      expect(blockerWords(blocker, null, i18n.t.bind(i18n)).text).toBe(english);
      await setLocale('zh-CN');
      expect(blockerWords(blocker, null, i18n.t.bind(i18n)).text).toBe(chinese);
      expect(blockerWords({ code: 'runtime_limit.other', message: 'Copy files first.' }, null, i18n.t.bind(i18n)).text).toBe('Copy files first.');
    } finally {
      await setLocale('en');
    }
  });

  it('says what stops a plan and what would lift it, in the words of the page', () => {
    const plan = { to: { provider: { name: 'OpenAI key' }, model: 'gpt-5.4', effort: 'high' } } as never;
    const blocker = (code: string, more: Partial<PlanBlocker> = {}): PlanBlocker => ({ code, message: `the server says ${code}`, ...more });
    expect(blockerWords(blocker('model_not_allowed'), plan, t)).toEqual({ text: 'gpt-5.4 is not a model OpenAI key offers', lifts: 'Choose a model it offers.' });
    expect(blockerWords(blocker('already_running'), null, t).lifts).toBe('Wait for it to end, or look at how far it has come.');
    expect(blockerWords(blocker('nothing_to_change'), null, t).text).toBe('This would change nothing');
    // A code that has no words here keeps the server's own sentence, and promises nothing.
    expect(blockerWords(blocker('something_new'), null, t)).toEqual({ text: 'the server says something_new', lifts: null });
    // Missing access says whom to ask only when it names what is missing.
    expect(blockerWords(blocker('access'), null, t).lifts).toBeNull();
    expect(blockerWords(blocker('access', { missing: [{ ask: [{ id: 'u1' }] } as never] }), null, t).lifts).toBe('Ask the people named below, or choose something you may use.');
  });

  it('asks the people the refusal names', () => {
    const ada = { id: 'u1', display_name: 'Ada' };
    const grace = { id: 'u2', display_name: 'Grace' };
    expect(askOf({ code: 'access', message: 'no', missing: [{ ask: [ada] }, { ask: [grace] }] } as unknown as PlanBlocker)).toEqual([ada, grace]);
    expect(askOf({ code: 'unbound', message: 'no' })).toEqual([]);
  });

  it('says a span of seconds as a range in the unit that fits it, in the language of the reader', () => {
    expect(rangeWords(20, 45, 'en')).toBe('20–45 sec');
    expect(rangeWords(360, 540, 'en')).toBe('6–9 min');
    expect(rangeWords(10, 10, 'en')).toBe('10 sec');
    expect(rangeWords(7_200, 9_000, 'en')).toBe('2–3 hr');
    expect(rangeWords(20, 45, 'zh-CN')).toBe('20-45秒');
  });

  it('reads a model with its effort and a history by how well it was kept', () => {
    expect(modelWords('gpt-5.4', 'high')).toBe('gpt-5.4 (high)');
    expect(modelWords('gpt-5.4', null)).toBe('gpt-5.4');
    expect(fidelityKey('none', 'lossless')).toBe('none');
    expect(fidelityKey('native_fork', 'lossless')).toBe('lossless');
  });
});

describe('a job', () => {
  const job = (phases: [string, string][], log: string | null = null): Job => ({ log, progress: { phase: 'verify', phases: phases.map(([name, state]) => ({ name, state })) } }) as unknown as Job;

  it('cannot be undone once its commit has happened', () => {
    expect(committed(job([['lock', 'done'], ['commit', 'running']]))).toBe(false);
    expect(committed(job([['lock', 'done'], ['commit', 'done'], ['cleanup', 'running']]))).toBe(true);
    expect(committed(job([]))).toBe(false);
  });

  it('shows the checks of the new side only as the job printed them, one to a line', () => {
    expect(checksOf(job([], 'environment attached\n\n files present \n'))).toEqual(['environment attached', 'files present']);
    expect(checksOf(job([], null))).toEqual([]);
  });
});

describe('the way into the wizard and into a side thread', () => {
  beforeEach(() => {
    useChangeHost.setState({ current: null, open: false });
    useSideAsk.setState({ current: null, open: false });
  });

  it('opens what was asked for, opens it anew each time it is asked, and lets go of it once it is closed', () => {
    openChange({ kind: 'thread', id: 't-1', name: 'Main' }, { kind: 'fork', toTurn: 2 });
    const first = useChangeHost.getState();
    expect(first.open).toBe(true);
    expect(first.current).toMatchObject({ target: { kind: 'thread', id: 't-1' }, options: { kind: 'fork', toTurn: 2 } });
    openChange({ kind: 'thread', id: 't-1', name: 'Main' }, { kind: 'fork', toTurn: 2 });
    expect(useChangeHost.getState().current?.asked).toBeGreaterThan(first.current?.asked ?? 0);
    useChangeHost.getState().hide();
    expect(useChangeHost.getState().open).toBe(false);
  });

  it('opens the one line that starts a side thread, from a message or from the menu of the thread', () => {
    openSideAsk({ threadId: 't-1', threadName: 'Main', toTurn: 2, text: '' });
    expect(useSideAsk.getState()).toMatchObject({ open: true, current: { threadId: 't-1', toTurn: 2 } });
    useSideAsk.getState().hide();
    expect(useSideAsk.getState().open).toBe(false);
  });
});
