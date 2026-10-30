/** Each build's existing status follows both checks and names the native case that failed. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { BuildStatus, ChannelReadStatus } from '@/features/admin/HarnessVersions';
import type { HarnessBuild, HarnessChannelsStatus } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const openHelp = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openHelp }));
afterEach(async () => { cleanup(); openHelp.mockClear(); await setLocale('en'); });

const build: HarnessBuild = {
  kind: 'codex', version: '0.170.0', os: 'linux', arch: 'x86_64', url: 'https://example.com/build', sha256: 'c'.repeat(64), is_default: false, source: 'channel', status: 'checking',
  check: { at: '2026-01-01T00:00:00.000Z', machine: 'Check host', missing: [], off: [], error: null, stage: 'interface', cases: [] },
};

for (const locale of ['en', 'zh-CN'] as const) it(`keeps unavailable native history labelled and opens its method by keyboard (${locale})`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<BuildStatus build={{ ...build, status: 'checked', check: { ...build.check!, stage: 'complete', off: ['import_native'], cases: [{
    name: 'history.native_import', passed: true, feature: 'import_native', error: null,
    unsupported: 'The exported archive depends on its original home', elapsed_ms: 12,
  }] } }} />);
  const label = i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.nativeImport') });
  const trigger = screen.getByRole('button', { name: label });
  await user.tab();
  expect(trigger).toHaveFocus();
  await user.keyboard('{Enter}');
  const details = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(details).toHaveTextContent(`${i18n.t('programVersions.checks.nativeImport')} · ${locale === 'en' ? 'Unavailable' : '不可用'}`);
  expect(details).toHaveTextContent('The exported archive depends on its original home');
  expect(details).not.toHaveTextContent('history.native_import');
  await user.click(screen.getByRole('button', { name: locale === 'en' ? 'History support' : '历史支持' }));
  expect(openHelp).toHaveBeenCalledExactlyOnceWith('carry-a-thread', 'read-the-method');
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`follows interface, conformance and the first failing case in the existing status (${locale})`, async () => {
  await setLocale(locale);
  const view = show(<BuildStatus build={build} />);
  expect(screen.getByText(locale === 'en' ? 'Checking interface…' : '正在检查接口…')).toBeVisible();
  const second = { ...build, check: { ...build.check!, stage: 'conformance' as const } };
  view.rerender(<BuildStatus build={second} />);
  expect(screen.getByText(locale === 'en' ? 'Checking conformance…' : '正在检查兼容性…')).toBeVisible();
  view.rerender(<BuildStatus build={{ ...second, status: 'failed', check: { ...second.check, stage: 'complete', cases: [{ name: 'user_input', passed: false, feature: 'approvals.user_input', error: 'No native question', elapsed_ms: 12 }] } }} />);
  expect(screen.getByText(i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }))).toBeVisible();
  view.rerender(<BuildStatus build={{ ...second, status: 'checked', check: { ...second.check, stage: 'complete' } }} />);
  expect(screen.getByText(locale === 'en' ? 'Checked' : '已检查')).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`says why channels stay local and keeps the last read as the shared time (${locale})`, async () => {
  await setLocale(locale);
  const state: HarnessChannelsStatus = { last_read_at: null, reason: 'shipped', interval_hours: 24, requested: [] };
  const view = show(<ChannelReadStatus value={state} />);
  expect(view.container).toHaveTextContent(i18n.t('admin.harnesses.channels.shipped'));
  expect(view.container.querySelector('[data-state-line]')).not.toBeNull();
  view.rerender(<ChannelReadStatus value={{ ...state, reason: 'disabled', interval_hours: 0 }} />);
  expect(view.container).toHaveTextContent(i18n.t('admin.harnesses.channels.disabled'));
  const at = new Date(Date.now() - 120_000).toISOString();
  const thirdName = locale === 'en' ? 'Research assistant' : '研究助手';
  const title = (kind: string) => kind === 'codex' ? 'Codex' : thirdName;
  view.rerender(<ChannelReadStatus value={{ ...state, reason: 'newer', requested: ['codex', 'research-agent'], last_read_at: at }} title={title} />);
  expect(view.container).toHaveTextContent(i18n.t('admin.harnesses.channels.lastRead'));
  expect(view.container).toHaveTextContent(i18n.t('admin.harnesses.channels.newer', { kinds: `Codex · ${thirdName}` }));
  expect(view.container).not.toHaveTextContent('research-agent');
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', at);
});

for (const locale of ['en', 'zh-CN'] as const) for (const [name, feature] of [['elicitation', 'approvals.mcp_elicitation'], ['nested_subagent', 'subagents']] as const) it(`names ${name} and only its disabled feature in the row details (${locale})`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<BuildStatus build={{ ...build, status: 'failed', check: { ...build.check!, stage: 'complete', off: [feature], cases: [{ name, passed: false, feature, error: 'Native behavior was not observed', elapsed_ms: 12 }] } }} />);
  const label = i18n.t('programVersions.offInBuild', { feature: i18n.t(name === 'elicitation' ? 'programVersions.disabledFeatures.toolQuestions' : 'programVersions.disabledFeatures.subagents') });
  await user.click(screen.getByRole('button', { name: label }));
  const details = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(details).toHaveTextContent(label);
  expect(details).toHaveTextContent(`${i18n.t(name === 'elicitation' ? 'programVersions.checks.elicitation' : 'programVersions.checks.subagent')} · ${i18n.t('admin.harnesses.status.failed')}`);
  expect(details).not.toHaveTextContent(feature);
  expect(details.querySelector('[data-build-case] .text-text-muted')).toHaveTextContent('Native behavior was not observed');
  expect(details).not.toHaveTextContent('dynamic_tools');
  expect(details).not.toHaveTextContent(name === 'elicitation' ? 'subagents' : 'approvals.mcp_elicitation');
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses the program's catalogue for a third program's check and feature (${locale})`, async () => {
  await setLocale(locale);
  const catalogue = {
    cases: [{ name: 'plugin.observation', title: { en: 'Observes handoffs', 'zh-CN': '检查交接' }, feature: 'plugin.feature' }],
    features: [{ id: 'plugin.feature', title: { en: 'handoff support', 'zh-CN': '交接支持' } }],
    runner: { en: 'Returns a report', 'zh-CN': '返回报告' },
  };
  const failed = { ...build, kind: 'research-agent', status: 'failed' as const, check: { ...build.check!, stage: 'complete' as const, off: ['plugin.feature'], cases: [{ name: 'plugin.observation', passed: false, feature: 'plugin.feature', error: 'No handoff', elapsed_ms: 12 }] } };
  show(<BuildStatus build={failed} name={locale === 'zh-CN' ? '研究助理' : 'Research assistant'} catalogue={catalogue} />);
  const label = i18n.t('programVersions.offInBuild', { feature: locale === 'zh-CN' ? '交接支持' : 'handoff support' });
  await userEvent.click(screen.getByRole('button', { name: label }));
  const card = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(card).toHaveTextContent(locale === 'zh-CN' ? '检查交接' : 'Observes handoffs');
  expect(card).toHaveTextContent('No handoff');
  expect(card).not.toHaveTextContent('plugin.observation');
  expect(card).not.toHaveTextContent('plugin.feature');
});

it('opens the existing result trigger by keyboard and tap and closes the top card with Escape', async () => {
  const user = userEvent.setup();
  show(<BuildStatus build={{ ...build, status: 'failed', check: { ...build.check!, stage: 'complete', cases: [{ name: 'user_input', passed: false, feature: 'approvals.user_input', error: 'No native question', elapsed_ms: 12 }] } }} />);
  const trigger = screen.getByRole('button', { name: i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }) });
  await user.tab();
  expect(trigger).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') })).toHaveTextContent('No native question');
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(trigger);
  expect(screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') })).toBeVisible();
});

it('uses the shared error alert and its single retry action when channel metadata cannot be read', async () => {
  const user = userEvent.setup();
  const retry = vi.fn();
  show(<ChannelReadStatus value={undefined} failed error={new Error('offline')} onRetry={retry} />);
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('admin.harnesses.channels.unavailable'));
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(retry).toHaveBeenCalledOnce();
});

for (const locale of ['en', 'zh-CN'] as const) it(`marks a recorded checksum refusal separately from a compatibility failure (${locale})`, async () => {
  await setLocale(locale);
  show(<BuildStatus build={{ ...build, status: 'failed', check: { ...build.check!, stage: 'complete', error: 'refused: checksum mismatch' } }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.refused') }));
  expect(screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') })).toHaveTextContent('refused: checksum mismatch');
});

for (const locale of ['en', 'zh-CN'] as const) for (const [kind, carry] of [['codex', 'items'], ['claude-code', 'handoff'], ['research-agent', 'items'], ['research-agent', 'handoff']] as const) it(`explains unavailable native history before its muted diagnostic (${locale}/${kind}/${carry})`, async () => {
  await setLocale(locale);
  const result = { name: 'history.native_import', passed: true, feature: 'import_native', error: null, unsupported: 'The exported archive depends on its original home', elapsed_ms: 12 };
  const checked = { ...build, kind, carry, status: 'checked' as const, check: { ...build.check!, stage: 'complete' as const, off: ['import_native'], cases: [result] } };
  show(<BuildStatus build={checked} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.nativeImport') }) }));
  const card = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(card).toHaveTextContent(i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.nativeImport') }));
  expect(card).toHaveTextContent(i18n.t(carry === 'items' ? 'programVersions.fallback.neutralCarry' : 'programVersions.fallback.handoff'));
  expect(card).toHaveTextContent(`${i18n.t('programVersions.checks.nativeImport')} · ${i18n.t('programVersions.caseUnavailable')}`);
  expect(card).not.toHaveTextContent('import_native');
  expect(card).not.toHaveTextContent('history.native_import');
  expect(card.querySelector('[data-build-case] .text-text-muted')).toHaveTextContent(result.unsupported);
  expect(card.textContent.indexOf(i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.nativeImport') }))).toBeLessThan(card.textContent.indexOf(result.unsupported));
});

it('keeps an unknown case in monospace while never showing an unknown feature identifier', async () => {
  show(<BuildStatus build={{ ...build, status: 'failed', check: { ...build.check!, stage: 'complete', off: ['plugin.private_feature', 'toString'], cases: [{ name: 'plugin.observation', passed: false, feature: 'plugin.private_feature', error: 'No matching observation', elapsed_ms: 12 }, { name: 'constructor', passed: true, feature: null, error: null, elapsed_ms: 12 }] } }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.other') }) }));
  const card = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(within(card).getByText('plugin.observation').tagName).toBe('CODE');
  expect(within(card).getByText('constructor')).toHaveClass('font-mono');
  expect(card).not.toHaveTextContent('plugin.private_feature');
  expect(card).not.toHaveTextContent('toString');
});
