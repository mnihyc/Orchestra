/** Email setup stays visible with its native prerequisite; an existing method remains removable. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Users from '@/api/users';
import { Security } from '@/features/settings/personal/Security';
import type { Me, SessionInfo } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { me, meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  person: vi.fn<() => Promise<Me>>(),
  send: vi.fn<typeof Users.sendEmailCode>(),
  enable: vi.fn<typeof Users.enableEmailCode>(),
  disable: vi.fn<typeof Users.disableEmailCode>(),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(),
  meQuery: { queryKey: ['me'], queryFn: calls.person },
  metaQuery: { queryKey: ['meta'], queryFn: () => Promise.resolve(meta()) },
}));
vi.mock('@/api/users', async (load) => ({ ...await load<typeof Users>(),
  sessionsPageQuery: (q: string) => ({ queryKey: ['sessions', 'pages', q], initialPageParam: null, queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => null }),
  sessionsQuery: { queryKey: ['sessions'], queryFn: (): Promise<SessionInfo[]> => Promise.resolve([]) },
  sendEmailCode: calls.send, enableEmailCode: calls.enable, disableEmailCode: calls.disable,
}));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));

const words = {
  en: { mail: 'Code by mail', policy: 'Not allowed by sign-in policy', address: 'Confirmed email address required',
    setup: 'Use a code by mail as a second step', remove: 'Switch off the code by mail' },
  'zh-CN': { mail: '邮件验证码', policy: '登录策略不允许', address: '需要已确认的邮箱',
    setup: '使用邮件验证码作为第二步验证', remove: '关闭邮件验证码' },
} as const;

beforeEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps Email disabled with its policy reason and sends nothing in ${locale}`, async () => {
    await setLocale(locale);
    calls.person.mockResolvedValue(me(1, 'ada', 'Ada', { email_code: [], email_code_second_step: false }));
    show(<Security />);
    const mail = await screen.findByRole('switch', { name: words[locale].mail });
    expect(mail).toBeDisabled();
    expect(mail).not.toBeChecked();
    expect(mail).toHaveAccessibleDescription(words[locale].policy);
    expect(screen.getByText(words[locale].policy)).toBeVisible();
    await userEvent.setup().click(mail);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(calls.send).not.toHaveBeenCalled();
    expect(calls.enable).not.toHaveBeenCalled();
    expect(calls.disable).not.toHaveBeenCalled();
  });

  it.each([null, 'unconfirmed@example.test'])(`keeps the verified-address prerequisite at Email (%s, ${locale})`, async (email) => {
    await setLocale(locale);
    calls.person.mockResolvedValue(me(1, 'ada', 'Ada', { email, email_verified: false, email_code: [] }));
    show(<Security />);
    const mail = await screen.findByRole('switch', { name: words[locale].mail });
    expect(mail).toBeDisabled();
    expect(mail).toHaveAccessibleDescription(words[locale].address);
    expect(screen.getByText(words[locale].address)).toBeVisible();
    expect(screen.queryByText(words[locale].policy)).not.toBeInTheDocument();
  });

  it(`keeps allowed Email setup behind its existing proof dialog in ${locale}`, async () => {
    await setLocale(locale);
    calls.person.mockResolvedValue(me(1, 'ada', 'Ada', { email_code: ['second_step'] }));
    show(<Security />);
    const mail = await screen.findByRole('switch', { name: words[locale].mail });
    expect(mail).toBeEnabled();
    await userEvent.setup().click(mail);
    expect(await screen.findByRole('dialog', { name: words[locale].setup })).toBeVisible();
    expect(calls.send).not.toHaveBeenCalled();
    expect(calls.enable).not.toHaveBeenCalled();
  });

  it(`keeps existing Email removable after policy stops allowing setup in ${locale}`, async () => {
    await setLocale(locale);
    calls.person.mockResolvedValue(me(1, 'ada', 'Ada', { email_code: [], email_code_second_step: true }));
    show(<Security />);
    const mail = await screen.findByRole('switch', { name: words[locale].mail });
    expect(mail).toBeChecked();
    expect(mail).toBeEnabled();
    expect(screen.queryByText(words[locale].policy)).not.toBeInTheDocument();
    await userEvent.setup().click(mail);
    expect(await screen.findByRole('dialog', { name: words[locale].remove })).toBeVisible();
    expect(calls.disable).not.toHaveBeenCalled();
  });
}

// EN: Security → Second step → Code by mail; read the disabled policy/address reason, or open the existing setup/removal dialog.
// 中文：安全 → 第二步验证 → 邮件验证码；查看策略或邮箱限制，或打开现有的设置／关闭对话框。
