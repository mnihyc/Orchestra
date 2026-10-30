/** Status facts use plain words, localized observation times and only limits that need attention. */
import { beforeAll, expect, it } from 'vitest';
import { isolationWords, nearLimit, workspaceWords } from '@/features/workbench/statusFacts';
import { changeTotals } from '@/features/workbench/Summaries';
import type { CapStatus, EnvironmentChange, GitStatus } from '@/generated/api-types';
import { i18n, loadSearchCatalogues } from '@/i18n';
import { statusContext } from './status-items-fixtures';

beforeAll(async () => { await loadSearchCatalogues(); });
const t = i18n.t.bind(i18n);

const observed = { attention: { uncommitted: null, unpushed: null, failed: false, stale: false, reason: null, as_of: '2026-10-15T03:05:00.000Z' } };
const unread = (reason: GitStatus['reason']): GitStatus => ({ available: false, reason, branch: null, head: null, ahead: null, behind: null, changes: [], remotes: [], upstream: null, repository_dir: null, status_hash: null });

it('has nothing to say for a plain directory, whether or not Git was asked', () => {
  const plain = { attention: { ...observed.attention, reason: 'not_a_repository' } };
  expect(workspaceWords(plain, undefined, t, { locale: 'en', timeZone: 'UTC' })).toBeNull();
  expect(workspaceWords(plain, unread('not_a_repository'), t, { locale: 'en', timeZone: 'UTC' })).toBeNull();
});

it('leaves an unreadable Git status to the Git view rather than the bar', () => {
  for (const reason of ['git_missing', 'offline', null] as const) expect(workspaceWords(observed, unread(reason), t, { locale: 'en', timeZone: 'UTC' })).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('names a repository branch with its localized observation time in %s', (language) => {
  const translate = i18n.getFixedT(language);
  const git: GitStatus = { ...unread(null), available: true, branch: 'main', head: '0123456789abcdef' };
  const words = workspaceWords(observed, git, translate, { locale: language, timeZone: 'UTC' });
  expect(words?.label).toBe('main');
  expect(words?.hint).toContain(translate('workbench.status.checked', { fact: 'main', time: '' }).replace(/\s*$/, ''));
  expect(words?.hint).not.toContain('T03:05');
  expect(workspaceWords(observed, { ...git, branch: null }, translate, { locale: language, timeZone: 'UTC' })?.label).toBe(translate('inspector.git.detached'));
});

it('keeps the isolation sentence and its explanation together', () => {
  const binding = statusContext().thread.binding;
  if (binding === null) throw new Error('The bound thread fixture is required');
  const thread = { name: 'Work', environment: null, binding: {
    ...binding, agent: { ...binding.agent, name: 'Claude Code' }, isolation: { ...binding.isolation, type: 'separated', tools: 'executor', tool_set: 'plain', executor_ref: { kind: 'exec', title: { en: 'Executor' }, tool_set: { name: 'plain', title: { en: 'Plain tools' }, summary: { en: 'Plain executor tools' } } }, credential_path: 'forwarded', harness_runs_in: 'provider', orch_tools: { offered: true, reason: null } },
  } } as const;
  const words = isolationWords(thread, t);
  expect(words?.label).toBe('Run with provider');
  expect(words?.hint).toContain('Claude Code uses Plain tools in the environment.');
  expect(words?.hint).toContain('credential stays with the provider');
  expect(words?.warning).toBe(false);
});

it.each(['en', 'zh-CN'] as const)('names the bound agent and the declared executor tool set in %s', (language) => {
  const binding = statusContext().thread.binding;
  if (binding === null) throw new Error('The bound thread fixture is required');
  const thread = { name: 'Login', environment: null, binding: {
    ...binding, agent: { ...binding.agent, name: 'Scripted agent' }, isolation: { ...binding.isolation, type: 'separated', tools: 'executor', tool_set: 'research', executor_ref: { kind: 'exec', title: { en: 'Executor' }, tool_set: { name: 'research', title: { en: 'Research tools', 'zh-CN': '研究工具' }, summary: { en: 'Tools for research', 'zh-CN': '研究工具集' } } }, credential_path: 'forwarded', harness_runs_in: 'provider', orch_tools: { offered: true, reason: null } },
  } } as const;
  const translate = i18n.getFixedT(language);
  const name = language === 'en' ? 'Research tools' : '研究工具';
  const words = isolationWords(thread, translate, language);
  expect(words?.hint).toContain(translate('thread.isolation.tools.namedFor', { agent: 'Scripted agent', name }));
  expect(words?.hint).not.toContain('Claude Code');
  const own = isolationWords({ ...thread, binding: { ...thread.binding, isolation: { ...thread.binding.isolation, tools: 'own' } } }, translate, language);
  expect(own?.hint).not.toContain(translate('thread.isolation.tools.namedFor', { agent: 'Scripted agent', name }));
});

it('keeps a healthy or replaced cap out of the status and gives a reached cap priority', () => {
  const cap = (state: CapStatus['state'], enabled = true, replaced_by: string | null = null) => ({ cap: { enabled }, state, replaced_by }) as CapStatus;
  expect(nearLimit([cap('ok'), cap('warning', false), cap('reached', true, 'replacement')])).toBeNull();
  const warning = cap('warning');
  const reached = cap('reached');
  expect(nearLimit([warning])).toBe(warning);
  expect(nearLimit([warning, reached])).toBe(reached);
});

it('summarizes only the changes in the supplied thread result', () => {
  const changes = [{ path: 'app.ts', added: 12, removed: 1 }, { path: 'util.ts', added: 2, removed: 3 }] as EnvironmentChange[];
  expect(changeTotals(changes)).toEqual({ files: 2, added: 14, removed: 4 });
});
