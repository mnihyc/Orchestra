/** Page acts stay directly visible; their ordinary permission and native-link behavior is retained. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { NodeActionButtons } from '@/features/navigation/NodeMenu';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) {
  it(`shows acts once without a hidden menu in ${locale}/${theme}`, async () => {
    await setLocale(locale);
    document.documentElement.setAttribute('data-theme', theme);
    const open = vi.fn();
    const denied = vi.fn();
    const user = userEvent.setup();
    show(<NodeActionButtons label="Org actions" entries={[
      { id: 'open', label: i18n.t('places.menu.open'), onSelect: open },
      { id: 'rename', label: i18n.t('places.menu.rename'), disabled: i18n.t('places.menu.checking'), onSelect: denied },
      { id: 'copy', label: i18n.t('places.menu.copyLink'), href: '/org/acme', onSelect: () => undefined },
    ]} />);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('places.menu.open') })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: i18n.t('places.menu.open') }));
    expect(open).toHaveBeenCalledTimes(1);
    const blocked = screen.getByRole('button', { name: i18n.t('places.menu.rename') });
    expect(blocked).toHaveAttribute('aria-disabled', 'true');
    await user.click(blocked);
    expect(denied).not.toHaveBeenCalled();
    expect(screen.getByRole('link', { name: i18n.t('places.menu.copyLink') })).toHaveAttribute('href', '/org/acme');
  });
}
