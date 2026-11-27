/** A command approval offers what the agent program's own prompt offers: allow once, allow commands like it for this
 * thread with the rule shown first, and a refusal that tells the agent what to do instead, all in one compact row. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { decisionOfOption, OptionButtons } from '@/features/approvals/Harness';
import type { ApprovalOption } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const deny: ApprovalOption = { id: 'deny', label: 'Deny', style: 'deny', scope: null, rule: null, note: true };
const options: ApprovalOption[] = [
  { id: 'allow_once', label: 'Allow once', style: 'allow', scope: 'once', rule: null },
  { id: 'allow_thread', label: 'Allow for this thread', style: 'allow', scope: 'thread', rule: { type: 'command_prefix', argv: ['find'], cwd: '/work/env1' } },
  deny,
];
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`shows the kept rule before it is chosen and sends a refusal with the person's words (${locale})`, async () => {
  await setLocale(locale);
  const chosen = vi.fn<(option: ApprovalOption, note?: string) => void>();
  show(<OptionButtons options={options} disabled={false} pressed={null} onChoose={chosen} />);
  expect(screen.getByText(i18n.t('approvals.matcher.prefix', { command: 'find', cwd: '/work/env1' }))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('approvals.allowThread') })).toBeVisible();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('approvals.denyWithNote') }));
  const words = screen.getByRole('textbox', { name: i18n.t('approvals.denyNote') });
  expect(words).toHaveFocus();
  // Escape goes back to the buttons and decides nothing.
  await user.keyboard('{Escape}');
  expect(chosen).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: i18n.t('approvals.allowOnce') })).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('approvals.denyWithNote') }));
  await user.type(screen.getByRole('textbox', { name: i18n.t('approvals.denyNote') }), 'Use rg instead{Enter}');
  expect(chosen).toHaveBeenCalledWith(deny, 'Use rg instead');
  expect(decisionOfOption(deny, 'Use rg instead')).toEqual({ option_id: 'deny', note: 'Use rg instead' });
});

it('offers no place for words where the agent would not read them', () => {
  const plain = options.map((option) => ({ ...option, note: false }));
  show(<OptionButtons options={plain} disabled={false} pressed={null} onChoose={vi.fn()} />);
  expect(screen.queryByRole('button', { name: i18n.t('approvals.denyWithNote') })).not.toBeInTheDocument();
  expect(decisionOfOption({ ...deny, note: false }, 'ignored')).toEqual({ option_id: 'deny' });
});
