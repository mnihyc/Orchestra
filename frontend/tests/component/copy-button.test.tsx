/** Copy feedback and its accessible name remain intact in a shared inline control. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { CopyButton } from '@/ui/CopyButton';
import { show } from './support';

const clipboard = vi.hoisted(() => ({ write: vi.fn<(value: string) => Promise<boolean>>() }));
vi.mock('@/ui/clipboard', () => ({ writeToClipboard: (value: string) => clipboard.write(value) }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const allowed of [true, false]) {
  it(`keeps copy feedback and its unchanged name ${locale}/${allowed ? 'copied' : 'refused'}`, async () => {
    await setLocale(locale); clipboard.write.mockResolvedValue(allowed);
    const user = userEvent.setup(), label = i18n.t('vnc.menu.copyLine');
    show(<span className="inline-flex items-center">{i18n.t('vnc.startWith')} <code>DISPLAY=:1</code><CopyButton value="DISPLAY=:1" label={label} /></span>);
    const button = screen.getByRole('button', { name: label });
    expect(button).toHaveAccessibleName(label);
    await user.tab(); expect(button).toHaveFocus(); await user.keyboard('{Enter}');
    expect(clipboard.write).toHaveBeenCalledExactlyOnceWith('DISPLAY=:1');
    await waitFor(() => expect(within(button).getByRole('status')).toHaveTextContent(i18n.t(allowed ? 'common.copied' : 'common.copyRefused')));
    expect(button).toHaveAccessibleName(label);
  });
}
