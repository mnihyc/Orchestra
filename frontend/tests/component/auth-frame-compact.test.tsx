/** Auth has one product heading, one form action and compact language/theme names in every appearance. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import { AuthCard, AuthFrame } from '@/features/auth/Frame';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ title: vi.fn<(title: string) => void>(), preferences: vi.fn<ReturnType<typeof useAppearance.getState>['set']>() }));
vi.mock('@/lib/title', () => ({ usePageTitle: calls.title }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  metaQuery: { queryKey: ['compact-auth-meta'], queryFn: () => Promise.resolve(meta({ name: 'Workshop' })) },
}));

const initial = useAppearance.getState();
const initialTheme = document.documentElement.getAttribute('data-theme');
afterEach(async () => {
  cleanup();
  useAppearance.setState(initial);
  await setLocale('en');
  if (initialTheme === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.dataset.theme = initialTheme;
  calls.title.mockClear(); calls.preferences.mockClear();
});

const actions = {
  en: ['Sign in', 'Create account', 'Send recovery link', 'Verify code'],
  'zh-CN': ['登录', '创建账户', '发送恢复链接', '验证代码'],
} as const;

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it.each(actions[locale])(`says %s once with the product as the only h1 in ${locale}/${theme}`, async (action) => {
    await setLocale(locale);
    useAppearance.setState({ locale, theme, set: calls.preferences });
    document.documentElement.dataset.theme = theme;
    show(<AuthFrame><AuthCard title={action}><Button type="submit">{action}</Button></AuthCard></AuthFrame>);
    expect(await screen.findByRole('heading', { level: 1, name: 'Workshop' })).toBeVisible();
    expect(screen.getAllByRole('heading', { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole('heading', { name: action })).not.toBeInTheDocument();
    expect(screen.getAllByText(action, { exact: true })).toHaveLength(1);
    expect(screen.getByRole('button', { name: action })).toBeVisible();
    expect(calls.title).toHaveBeenCalledWith(action);

    const language = screen.getByLabelText(i18n.t('preferences.language'));
    expect(within(language).getByText('English', { exact: true })).toBeVisible();
    expect(within(language).getByText('简体中文', { exact: true })).toBeVisible();
    expect(language).not.toHaveTextContent(i18n.t('choiceCatalog.language.en.description'));
    const appearance = screen.getByLabelText(i18n.t('preferences.theme'));
    for (const value of ['system', 'light', 'dark'] as const) {
      expect(within(appearance).getByText(i18n.t(`choiceCatalog.theme.${value}.name`), { exact: true })).toBeVisible();
      expect(appearance).not.toHaveTextContent(i18n.t(`choiceCatalog.theme.${value}.description`));
    }
    const next = theme === 'light' ? 'dark' : 'light';
    const control = within(appearance).getByText(i18n.t(`choiceCatalog.theme.${next}.name`), { exact: true }).closest('button');
    if (control === null) throw new Error('The theme name has no control');
    await userEvent.click(control);
    expect(calls.preferences).toHaveBeenCalledWith({ theme: next });
  });
}
