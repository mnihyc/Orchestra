/** Missing server presentation never claims Idle; actual state reports still decide the icon. */
import userEvent from '@testing-library/user-event';
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ThreadStateChip, ThreadStateIcon, threadDisplayState } from '@/ui/ThreadState';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

afterEach(async () => {
  cleanup(); useAppearance.setState({ locale: 'en' }); await setLocale('en');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps a missing chip explicit and its hint available on keyboard focus in ${locale}`, async () => {
    useAppearance.setState({ locale }); await setLocale(locale);
    const user = userEvent.setup();
    const view = show(<ThreadStateChip display={undefined} />);
    expect(threadDisplayState(undefined)).toBeUndefined();
    expect(threadDisplayState(null)).toBeUndefined();
    const absent = screen.getByRole('img', { name: i18n.t('threadStates.notReported') });
    expect(absent).toHaveTextContent('—');
    expect(screen.queryByText(i18n.t('state.idle'))).not.toBeInTheDocument();
    await user.tab();
    expect(absent).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('threadStates.notReported'));
    view.rerender(<ThreadStateChip display={executionDisplay('idle')} />);
    expect(screen.getByText(i18n.t('state.idle'))).toBeVisible();
    expect(screen.queryByText('—')).not.toBeInTheDocument();
  });

  it(`does not infer an icon while a missing row is replaced by actual running facts in ${locale}`, async () => {
    useAppearance.setState({ locale }); await setLocale(locale);
    const view = show(<ThreadStateIcon display={null} />);
    expect(screen.getByRole('img', { name: i18n.t('threadStates.notReported') })).toHaveTextContent('—');
    expect(document.querySelector('[data-work-state="idle"]')).not.toBeInTheDocument();
    view.rerender(<ThreadStateIcon display={executionDisplay('running')} />);
    expect(screen.getByRole('img', { name: i18n.t('state.running') })).toHaveAttribute('data-work-state', 'running');
    expect(screen.queryByText('—')).not.toBeInTheDocument();
  });
}
