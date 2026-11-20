/** The arrangement heading reuses its one question to reach the full documentation. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ArrangementSection } from '@/features/start/Arrangement';
import { NO_PICKS, resolve } from '@/features/start/choices';
import { i18n, setLocale } from '@/i18n';
import { options } from '../unit/start-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => calls.open }));
afterEach(async () => { cleanup(); calls.open.mockReset(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`opens the documented arrangement from its section heading without changing the selection (${locale})`, async () => {
  await setLocale(locale); const change = vi.fn(), user = userEvent.setup();
  const view = show(<ArrangementSection resolved={resolve(options(), NO_PICKS, { suggest: true })} refreshing={false} onChange={change} />);
  const name = i18n.t('help.hint.label', { term: i18n.t('start.arrangement.label') });
  const question = screen.getByRole('button', { name });
  expect(screen.getAllByRole('button', { name })).toHaveLength(1);
  expect(question).toHaveAttribute('data-help-article', 'register-your-program');
  expect(view.container).not.toHaveTextContent(i18n.t('help.hint.more'));
  await user.click(question);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.more') }));
  expect(calls.open).toHaveBeenCalledExactlyOnceWith('register-your-program', 'choose-an-arrangement');
  expect(change).not.toHaveBeenCalled();
});
