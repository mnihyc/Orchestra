/** Only the canonical executor-version refusal gets translated and links to the environment's Tools. */
import { cleanup, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { blockedDetail, waitingWords } from '@/features/timeline/waiting';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (loadOriginal) => ({ ...await loadOriginal<object>(), Link: ({ to, params, search, children, className }: { to: string; params?: { id: string }; search?: { tab: string }; children: ReactNode; className?: string }) =>
  <a className={className} href={`${to.replace('$id', params?.id ?? '')}${search === undefined ? '' : `?tab=${search.tab}`}`}>{children}</a> }));
const detail = (program = '0.164.0', agent = '0.165.0') => `Codex on Build machine is ${program}; this agent runs ${agent}. An environment manager can allow it on the environment's page.`;
function blocked(raw = detail(), manages = false): Thread {
  const native = failedStart().thread;
  return { ...native, state: 'blocked', lock: null, blocked: { reason: 'binding_invalid', detail: raw, sandbox: null },
    environment: native.environment === null ? null : { ...native.environment, status: 'running' },
    access: { ...native.access, permissions: manages ? ['thread.view', 'thread.interact', 'thread.manage'] : ['thread.view', 'thread.interact'] },
    environment_permissions: ['environment.view'],
  };
}
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  const line = (program = '0.164.0', agent = '0.165.0') => locale === 'en' ? detail(program, agent)
    : `Codex 在 Build machine 上的版本是 ${program === 'unknown' ? '未知' : program}；此代理运行的版本是 ${agent === 'unknown' ? '未知' : agent}。环境管理者可在环境页面允许使用该版本。`;
  it(`shows the exact known version mismatch without calling it a withdrawn arrangement (${locale})`, async () => {
    await setLocale(locale); const thread = blocked();
    expect(blockedDetail(detail(), i18n.getFixedT(locale))).toBe(line());
    expect(waitingWords(thread, i18n.getFixedT(locale))).toBe(line());
  });
  it(`localizes unknown versions in the known refusal (${locale})`, async () => {
    await setLocale(locale);
    expect(waitingWords(blocked(detail('unknown', 'unknown')), i18n.getFixedT(locale))).toBe(line('unknown', 'unknown'));
  });
  it(`keeps every other binding or vendor detail verbatim (${locale})`, async () => {
    await setLocale(locale);
    for (const raw of [`Native error: ${detail()}`, `${detail()} Vendor explanation.`, detail().replace('Codex on ', 'Claude Code on '), detail().replace('0.164.0', ''), 'The arrangement was withdrawn']) {
      expect(blockedDetail(raw, i18n.getFixedT(locale))).toBe(raw);
    }
  });
  for (const manages of [false, true]) it(`offers the canonical environment Tools link to a ${manages ? 'thread manager' : 'thread reader'} (${locale})`, async () => {
    await setLocale(locale); const thread = blocked(detail(), manages); show(<BlockedBanner thread={thread} onProceed={vi.fn()} />);
    expect(screen.getByText(line(), { exact: true })).toBeVisible();
    expect(screen.getByRole('link', { name: i18n.t('resourceFacts.openEnvironment') })).toHaveAttribute('href', `/resources/environments/${thread.environment?.id}?tab=tools`);
    expect(screen.queryByText(i18n.t('thread.blocked.manager', { who: thread.owner.display_name }))).not.toBeInTheDocument();
    if (manages) expect(screen.getByRole('button', { name: i18n.t('change.banner.switchProfile') })).toBeVisible();
    else expect(screen.queryByRole('button', { name: i18n.t('change.banner.switchProfile') })).not.toBeInTheDocument();
  });
}

it('does not invent an environment destination when its reference is absent', () => {
  const thread = { ...blocked(), environment: null, environment_permissions: [] }; show(<BlockedBanner thread={thread} onProceed={vi.fn()} />);
  expect(screen.queryByRole('link', { name: i18n.t('resourceFacts.openEnvironment') })).not.toBeInTheDocument();
});

it('keeps the existing profile recovery and owner hint for other invalid bindings', () => {
  const thread = blocked('The arrangement was withdrawn', false); show(<BlockedBanner thread={thread} onProceed={vi.fn()} />);
  expect(screen.getByText(i18n.t('thread.blocked.manager', { who: thread.owner.display_name }))).toBeVisible();
  expect(screen.queryByRole('link', { name: i18n.t('resourceFacts.openEnvironment') })).not.toBeInTheDocument();
});
