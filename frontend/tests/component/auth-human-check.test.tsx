/** The human check on the screens of a person without a session: nothing where it is off, a token for each try, and a box that is never left spent beside a button that stays off. */
import type { AnchorHTMLAttributes } from 'react';
import type * as Router from '@tanstack/react-router';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Auth from '@/api/auth';
import { ApiError } from '@/api/client';
import type { AuthCardProps } from '@/features/auth/Frame';
import { Login } from '@/features/auth/Login';
import { Recovery } from '@/features/auth/Recovery';
import { VerifyEmail } from '@/features/auth/VerifyEmail';
import type { HumanCheckAct, Meta, MetaHumanCheck } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import type * as HumanCheckLib from '@/lib/humanCheck';
import type { Callbacks } from '@/lib/humanCheck';
import { useAppearance } from '@/lib/appearance';
import { meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  meta: vi.fn<() => Promise<Meta>>(),
  signIn: vi.fn<typeof Auth.signIn>(),
  askCode: vi.fn<typeof Auth.askEmailCode>(),
  recover: vi.fn<typeof Auth.askRecovery>(),
  resend: vi.fn<typeof Auth.resendConfirmation>(),
}));

/** The vendor's widget, as a test drives it: boxes that were drawn, and the runs that were asked for. */
const widget = vi.hoisted(() => ({
  boxes: [] as { container: HTMLElement; on: Callbacks; resets: number; disposed: boolean }[],
  runs: [] as string[],
  loaded: [] as string[],
  fail: false,
}));

vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(),
  metaQuery: { queryKey: ['human-check-meta'], queryFn: calls.meta },
  signIn: calls.signIn, askEmailCode: calls.askCode, askRecovery: calls.recover, resendConfirmation: calls.resend,
}));
vi.mock('@/lib/humanCheck', async (load) => ({ ...await load<typeof HumanCheckLib>(),
  loadVendor: vi.fn((check: MetaHumanCheck) => {
    widget.loaded.push(check.provider);
    if (widget.fail) return Promise.reject(new Error('the script could not be loaded'));
    return Promise.resolve({
      render: (container: HTMLElement, on: Callbacks) => {
        const box = { container, on, resets: 0, disposed: false };
        container.dataset.fakeBox = 'drawn';
        widget.boxes.push(box);
        return {
          reset: () => { box.resets += 1; },
          dispose: () => { box.disposed = true; container.replaceChildren(); },
        };
      },
      run: (act: HumanCheckAct) => { widget.runs.push(act); return Promise.resolve(`unseen-${act}-${widget.runs.length}`); },
    });
  }),
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouter: () => ({ navigate: vi.fn(), history: { replace: vi.fn() } }),
  Link: ({ to, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));
vi.mock('@/features/auth/Frame', () => ({
  AuthCard: ({ children, foot }: AuthCardProps) => <section>{children}<footer>{foot}</footer></section>,
}));

const appearance = useAppearance.getState();
const ACTS: HumanCheckAct[] = ['sign_in', 'email_code', 'register', 'recovery', 'resend'];
const asking = (over: Partial<MetaHumanCheck> = {}): MetaHumanCheck => ({
  provider: 'hcaptcha', site_key: '10000000-ffff-ffff-ffff-000000000001', mode: 'checkbox', acts: ACTS, alternate_host: false, wait_secs: 20, ...over,
});
const problem = (code: 'auth.second_step_required' | 'auth.second_step_invalid' | 'auth.invalid_credentials', extra: Record<string, unknown> = {}) =>
  new ApiError({ type: `orch:error:${code}`, code, status: 403, title: code, detail: code, request_id: 'r', ...extra }, null);

/** The newest box on the page that was not taken away. */
const box = () => widget.boxes.filter((one) => !one.disposed).at(-1)!;
/** The person ticks the box. */
const tick = (token: string) => act(() => { box().on.token(token); });
const signInButton = () => screen.getByRole('button', { name: i18n.t('auth.login.submit') });

beforeEach(() => {
  widget.boxes.length = 0; widget.runs.length = 0; widget.loaded.length = 0; widget.fail = false;
  calls.meta.mockReset().mockResolvedValue(meta({ mail: true, email_code_login: true, human_check: asking() }));
  calls.signIn.mockReset();
  calls.askCode.mockReset().mockResolvedValue(undefined);
  calls.recover.mockReset().mockResolvedValue(undefined);
  calls.resend.mockReset().mockResolvedValue(undefined);
});
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

it('adds no script and draws nothing where the system asks no human check', async () => {
  calls.meta.mockResolvedValue(meta({ mail: true }));
  calls.signIn.mockResolvedValue({} as never);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-password');
  expect(signInButton()).toBeEnabled();
  expect(document.querySelector('[data-human-check]')).toBeNull();
  expect(document.querySelector('[data-human-check-notice]')).toBeNull();
  await user.click(signInButton());
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(1));
  // Sent as it always was: the body alone, and no token.
  expect(calls.signIn.mock.calls[0]).toEqual([{ login: 'reader', password: 'a-password' }]);
  expect(widget.loaded).toEqual([]);
});

it('adds no script on a screen whose acts the system does not ask', async () => {
  calls.meta.mockResolvedValue(meta({ mail: true, human_check: asking({ acts: ['register'] }) }));
  show(<Recovery />);
  await screen.findByLabelText(i18n.t('auth.fields.email'));
  expect(widget.loaded).toEqual([]);
  expect(document.querySelector('[data-human-check]')).toBeNull();
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`waits for the box, sends its token once, and draws it fresh again with one line that says why, in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    calls.signIn.mockRejectedValueOnce(problem('auth.invalid_credentials'));
    show(<Login onward={null} callbackError={null} />);
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
    await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'wrong-password');
    await waitFor(() => expect(widget.boxes.length).toBe(1));
    // The box is there and unticked: the button waits for it, and can say why.
    expect(signInButton()).toBeDisabled();
    expect(document.querySelector('[data-fake-box]')).not.toBeNull();
    tick('token-1');
    await waitFor(() => expect(signInButton()).toBeEnabled());
    await user.click(signInButton());
    await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(1));
    expect(calls.signIn.mock.calls[0]).toEqual([{ login: 'reader', password: 'wrong-password' }, 'token-1']);
    // The try is over: the box is fresh, the button waits for the box again, and one line says why.
    await waitFor(() => expect(box().resets).toBe(1));
    expect(await screen.findByText(i18n.t('auth.humanCheck.spent'))).toBeVisible();
    expect(signInButton()).toBeDisabled();
    expect(document.querySelector('[data-fake-box]')).not.toBeNull();
    // Ticking it again clears the line and lets the next try go with a token of its own.
    tick('token-2');
    await waitFor(() => expect(signInButton()).toBeEnabled());
    expect(screen.queryByText(i18n.t('auth.humanCheck.spent'))).not.toBeInTheDocument();
  });
}

it('walks a second step through three wrong codes: each try has a token of its own, the box is always there to tick, and the button is never left off beside a spent token', async () => {
  calls.signIn
    .mockRejectedValueOnce(problem('auth.second_step_required', { methods: ['totp'] }))
    .mockRejectedValueOnce(problem('auth.second_step_invalid'))
    .mockRejectedValueOnce(problem('auth.second_step_invalid'))
    .mockRejectedValueOnce(problem('auth.second_step_invalid'))
    .mockResolvedValueOnce({} as never);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'the-password');
  await waitFor(() => expect(widget.boxes.length).toBe(1));
  tick('first');
  await waitFor(() => expect(signInButton()).toBeEnabled());
  await user.click(signInButton());
  // The password was right and asks for a second step: the screen changes, the token of the first call is spent.
  expect(await screen.findByLabelText(i18n.t('auth.second.totpLabel'))).toBeVisible();
  const spentBox = () => document.querySelector('[data-human-check] [data-fake-box]');
  await waitFor(() => expect(spentBox()).not.toBeNull());
  expect(signInButton()).toBeDisabled();
  expect(screen.getByText(i18n.t('auth.humanCheck.spent'))).toBeVisible();
  const field = screen.getByLabelText(i18n.t('auth.second.totpLabel'));
  for (const [index, token] of ['second', 'third', 'fourth'].entries()) {
    await user.clear(field);
    await user.type(field, `12345${index}`);
    expect(signInButton()).toBeDisabled();
    tick(token);
    await waitFor(() => expect(signInButton()).toBeEnabled());
    await user.click(signInButton());
    await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(index + 2));
    // A wrong code spends its token: the box is fresh again, drawn, and the line says why.
    await screen.findByText(i18n.t('auth.second.wrong.totp'));
    expect(spentBox()).not.toBeNull();
    expect(signInButton()).toBeDisabled();
    expect(screen.getByText(i18n.t('auth.humanCheck.spent'))).toBeVisible();
  }
  tick('fifth');
  await user.clear(field);
  await user.type(field, '000000');
  await waitFor(() => expect(signInButton()).toBeEnabled());
  await user.click(signInButton());
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(5));
  // No token served two requests, and each call of the sign-in carried one.
  expect(calls.signIn.mock.calls.map(([, token]) => token)).toEqual(['first', 'second', 'third', 'fourth', 'fifth']);
  expect(calls.signIn.mock.calls[1]?.[0]).toMatchObject({ login: 'reader', password: 'the-password', totp: '123450' });
});

it('runs the check unseen for each try, draws no box, and shows the sentence the service asks for under the button', async () => {
  calls.meta.mockResolvedValue(meta({ mail: true, human_check: asking({ provider: 'recaptcha', mode: 'invisible', verification: 'siteverify' }) }));
  calls.signIn.mockRejectedValueOnce(problem('auth.invalid_credentials')).mockResolvedValueOnce({} as never);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-password');
  expect(signInButton()).toBeEnabled();
  expect(document.querySelector('[data-fake-box]')).toBeNull();
  expect(screen.getByText(i18n.t('auth.humanCheck.noticeRecaptcha'))).toBeVisible();
  await user.click(signInButton());
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(1));
  expect(calls.signIn.mock.calls[0]?.[1]).toBe('unseen-sign_in-1');
  // A refused password is typed again; the name stays.
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-password');
  await user.click(signInButton());
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(2));
  expect(calls.signIn.mock.calls[1]?.[1]).toBe('unseen-sign_in-2');
  // Unseen, nothing is said of a spent token.
  expect(screen.queryByText(i18n.t('auth.humanCheck.spent'))).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names hCaptcha's own sentence with its two links where it runs unseen, in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    calls.meta.mockResolvedValue(meta({ mail: true, human_check: asking({ mode: 'invisible' }) }));
    show(<Login onward={null} callbackError={null} />);
    const notice = await waitFor(() => {
      const found = document.querySelector<HTMLElement>('[data-human-check-notice="hcaptcha"]');
      expect(found).not.toBeNull();
      return found!;
    });
    const links = within(notice).getAllByRole('link');
    expect(links.map((link) => link.getAttribute('href'))).toEqual(['https://www.hcaptcha.com/privacy', 'https://www.hcaptcha.com/terms']);
    expect(links.every((link) => link.getAttribute('rel')?.includes('noreferrer'))).toBe(true);
  });
}

it('says that the check could not be loaded where its script cannot be, does not leave the button off, and says so with the try', async () => {
  widget.fail = true;
  calls.signIn.mockResolvedValue({} as never);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-password');
  expect(await screen.findByText(i18n.t('auth.humanCheck.unavailable'))).toBeVisible();
  await waitFor(() => expect(signInButton()).toBeEnabled());
  await user.click(signInButton());
  // No token could be had, and the try is made all the same with the word that says so: the server looks into it, and
  // lets the person in, or turns them away, as its setting says of a service that cannot be reached.
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(1));
  expect(calls.signIn.mock.calls[0]).toEqual([{ login: 'reader', password: 'a-password' }, 'unloaded']);
});

it('says so with the try where an unseen check could not be run, and makes it', async () => {
  widget.fail = true;
  calls.meta.mockResolvedValue(meta({ mail: true, human_check: asking({ provider: 'recaptcha', mode: 'invisible', verification: 'siteverify' }) }));
  calls.signIn.mockResolvedValue({} as never);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-password');
  await waitFor(() => expect(widget.loaded).toEqual(['recaptcha']));
  await user.click(signInButton());
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledTimes(1));
  expect(calls.signIn.mock.calls[0]).toEqual([{ login: 'reader', password: 'a-password' }, 'unloaded']);
  expect(await screen.findByText(i18n.t('auth.humanCheck.unavailable'))).toBeVisible();
});

it('asks the code by mail of the second step at its button, with a token of its own, where the box was spent by the first try', async () => {
  calls.signIn.mockRejectedValueOnce(problem('auth.second_step_required', { methods: ['email_code'] }));
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'the-password');
  await waitFor(() => expect(widget.boxes.length).toBe(1));
  tick('first');
  await waitFor(() => expect(signInButton()).toBeEnabled());
  await user.click(signInButton());
  expect(await screen.findByLabelText(i18n.t('auth.code.label'))).toBeVisible();
  // Nothing was sent by itself: the token of the first try is spent, and the box has to be ticked first.
  expect(calls.askCode).not.toHaveBeenCalled();
  const send = () => screen.getByRole('button', { name: i18n.t('auth.code.send') });
  expect(send()).toBeDisabled();
  tick('for-the-code');
  await waitFor(() => expect(send()).toBeEnabled());
  await user.click(send());
  await waitFor(() => expect(calls.askCode).toHaveBeenCalledTimes(1));
  expect(calls.askCode.mock.calls[0]).toEqual([{ login: 'reader', password: 'the-password' }, 'for-the-code']);
});

it('sends the token of a recovery request, and asks for none of the person where recovery is not among the acts', async () => {
  show(<Recovery />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.email')), 'reader@example.test');
  const submit = () => screen.getByRole('button', { name: i18n.t('auth.recover.submit') });
  await waitFor(() => expect(widget.boxes.length).toBe(1));
  expect(submit()).toBeDisabled();
  tick('recovery-token');
  await waitFor(() => expect(submit()).toBeEnabled());
  await user.click(submit());
  await waitFor(() => expect(calls.recover).toHaveBeenCalledTimes(1));
  expect(calls.recover.mock.calls[0]).toEqual(['reader@example.test', 'recovery-token']);
});

it('asks for the confirmation link again with the token of the box, on the screen of a link that has ended', async () => {
  const ended = new ApiError({ type: 'orch:error:gone', code: 'gone', status: 410, title: 'Gone', detail: 'This link has ended.', request_id: 'r' }, null);
  show(<VerifyEmail outcome={{ confirmed: false, error: ended }} onRetry={vi.fn()} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  const send = () => screen.getByRole('button', { name: i18n.t('auth.verify.resend') });
  await waitFor(() => expect(widget.boxes.length).toBe(1));
  expect(send()).toBeDisabled();
  tick('resend-token');
  await waitFor(() => expect(send()).toBeEnabled());
  await user.click(send());
  await waitFor(() => expect(calls.resend).toHaveBeenCalledTimes(1));
  expect(calls.resend.mock.calls[0]).toEqual(['reader', 'resend-token']);
});

it('tells a refused person in the words of the catalogue, in both languages', async () => {
  for (const locale of ['en', 'zh-CN'] as const) {
    await setLocale(locale);
    for (const code of ['required', 'failed', 'expired', 'unavailable']) {
      expect(i18n.t(`errors.human_check.${code}` as never)).not.toContain('errors.human_check');
    }
  }
  await setLocale('en');
  expect(i18n.t('errors.human_check.unavailable')).toBe('The human check cannot be reached. Try again shortly, or ask your administrator.');
});
