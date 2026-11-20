/** The human check card of Management → Sign-in: the choice of service first, the controls of that service after it, and a secret that is written and never read. */
import { QueryClient } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Admin from '@/api/admin';
import { HumanCheckCard } from '@/features/admin/HumanCheck';
import type { HumanCheckState, HumanCheckTestReply, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({
  settings: vi.fn<() => Promise<ResolvedSetting[]>>(),
  state: vi.fn<() => Promise<HumanCheckState>>(),
  set: vi.fn<typeof Admin.setHumanCheckSecret>(),
  drop: vi.fn<typeof Admin.deleteHumanCheckSecret>(),
  test: vi.fn<typeof Admin.testHumanCheck>(),
}));
vi.mock('@/api/admin', async (original) => ({ ...await original<typeof Admin>(),
  systemSettingsQuery: { queryKey: ['admin', 'settings'], queryFn: calls.settings },
  humanCheckQuery: { queryKey: ['admin', 'human-check'], queryFn: calls.state },
  setHumanCheckSecret: calls.set, deleteHumanCheckSecret: calls.drop, testHumanCheck: calls.test,
}));

const row = (key: string, value: unknown): ResolvedSetting => ({ key, value, source: { scope: 'system', node: null }, own: null, writable: true, locked_by: null, parts: [] });
const settings = (over: Record<string, unknown> = {}): ResolvedSetting[] => Object.entries({
  'human_check.provider': 'off', 'human_check.site_key': '', 'human_check.project_id': '', 'human_check.verification': 'siteverify',
  'human_check.mode': 'checkbox', 'human_check.acts': ['sign_in', 'email_code', 'register', 'recovery', 'resend'], 'human_check.min_score': 50,
  'human_check.when_unreachable': 'admit', 'human_check.alternate_host': false, ...over,
}).map(([key, value]) => row(key, value));
const t = (key: Parameters<typeof i18n.t>[0]) => i18n.t(key);

beforeEach(async () => {
  await setLocale('en');
  calls.settings.mockReset().mockResolvedValue(settings());
  calls.state.mockReset().mockResolvedValue({ secret_set: false, secret_changed_at: null, edge: null });
  calls.set.mockReset().mockResolvedValue({ secret_set: true, secret_changed_at: '2026-11-13T10:00:00Z', edge: null });
  calls.drop.mockReset().mockResolvedValue({ secret_set: false, secret_changed_at: null, edge: null });
  calls.test.mockReset().mockResolvedValue({ ok: true, error: null } satisfies HumanCheckTestReply);
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it('shows only the choice of service where none is chosen', async () => {
  show(<HumanCheckCard />);
  expect(await screen.findByLabelText(t('admin.signIn.humanCheck.service'))).toBeVisible();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.siteKey'))).not.toBeInTheDocument();
  expect(screen.queryByText(t('admin.signIn.humanCheck.secretNone'))).not.toBeInTheDocument();
});

it('shows hCaptcha its box or unseen, its secret, its acts and what to do when it cannot be reached, and no choice that is reCAPTCHA\'s', async () => {
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'hcaptcha', 'human_check.site_key': 'a-site-key' }));
  show(<HumanCheckCard />);
  expect(await screen.findByLabelText(t('admin.signIn.humanCheck.siteKey'))).toHaveValue('a-site-key');
  expect(screen.getByLabelText(t('admin.signIn.humanCheck.widget'))).toBeVisible();
  expect(screen.getByLabelText(t('admin.signIn.humanCheck.secret.hcaptcha'))).toBeVisible();
  expect(screen.getByLabelText(t('admin.signIn.humanCheck.unreachable'))).toBeVisible();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.verification'))).not.toBeInTheDocument();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.project'))).not.toBeInTheDocument();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.leastScore'))).not.toBeInTheDocument();
  const acts = screen.getByRole('group', { name: t('admin.signIn.humanCheck.acts') });
  expect(within(acts).getAllByRole('checkbox')).toHaveLength(5);
  // Nothing is said of the web edge where its policy has room for the service, or where nothing is known of it yet.
  expect(screen.queryByText(/web edge/)).not.toBeInTheDocument();
});

it('follows the way reCAPTCHA is asked: the project and the API key only for CreateAssessment', async () => {
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'recaptcha', 'human_check.site_key': 'a-site-key' }));
  const view = show(<HumanCheckCard />);
  expect(await screen.findByLabelText(t('admin.signIn.humanCheck.secret.siteverify'))).toBeVisible();
  expect(screen.getByLabelText(t('admin.signIn.humanCheck.leastScore'))).toBeVisible();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.project'))).not.toBeInTheDocument();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.widget'))).not.toBeInTheDocument();
  view.unmount();
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'recaptcha', 'human_check.site_key': 'a-site-key', 'human_check.verification': 'assessments' }));
  show(<HumanCheckCard />);
  expect(await screen.findByLabelText(t('admin.signIn.humanCheck.project'))).toBeVisible();
  expect(screen.getByLabelText(t('admin.signIn.humanCheck.secret.assessments'))).toBeVisible();
});

it('says that no site key is set, and keeps the secret write-only', async () => {
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'hcaptcha' }));
  show(<HumanCheckCard />);
  expect(await screen.findByText(t('admin.signIn.humanCheck.noKey'))).toBeVisible();
  expect(screen.getByText(t('admin.signIn.humanCheck.secretNone'))).toBeVisible();
  const test = screen.getByRole('button', { name: t('admin.signIn.humanCheck.test') });
  expect(test).toBeDisabled();
  const user = userEvent.setup();
  const field = screen.getByLabelText(t('admin.signIn.humanCheck.secret.hcaptcha'));
  expect(field).toHaveAttribute('type', 'password');
  await user.type(field, 'the-secret');
  await user.click(screen.getByRole('button', { name: t('admin.signIn.humanCheck.set') }));
  await waitFor(() => expect(calls.set).toHaveBeenCalledWith('the-secret'));
  // It is said to be kept, and the field is empty again: nothing of it is shown.
  expect(await screen.findByText(/Kept, set/)).toBeVisible();
  expect(field).toHaveValue('');
  expect(document.body.textContent).not.toContain('the-secret');
  await user.click(screen.getByRole('button', { name: t('admin.signIn.humanCheck.test') }));
  expect(await screen.findByText(t('admin.signIn.humanCheck.testOk'))).toBeVisible();
});

it('says in the words of the answer what went wrong in a test', async () => {
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'hcaptcha', 'human_check.site_key': 'a-site-key' }));
  calls.state.mockResolvedValue({ secret_set: true, secret_changed_at: '2026-11-13T10:00:00Z', edge: null });
  calls.test.mockResolvedValue({ ok: false, error: 'The service does not accept the secret. Set it again.' });
  show(<HumanCheckCard />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: t('admin.signIn.humanCheck.test') }));
  expect(await screen.findByText('The service does not accept the secret. Set it again.')).toBeVisible();
});

it('names the product the way Google names it, with reCAPTCHA beside it, and has the same words in Chinese', async () => {
  expect(t('choiceCatalog.humanCheckProvider.recaptcha.name')).toBe('Google Cloud Fraud Defense (reCAPTCHA)');
  await setLocale('zh-CN');
  expect(t('choiceCatalog.humanCheckProvider.recaptcha.name')).toContain('reCAPTCHA');
  expect(t('choiceCatalog.humanCheckProvider.recaptcha.name')).toContain('Google Cloud Fraud Defense');
});

/** A card on a client of its own, so that a test can change what the system's settings say, as a change of the provider does. */
const cardOn = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const switchTo = (over: Record<string, unknown>) => act(() => { client.setQueryData(['admin', 'settings'], settings({ 'human_check.site_key': 'a-site-key', ...over })); });
  return { client, switchTo };
};

it('shows the mode only for hCaptcha: choosing reCAPTCHA after an unseen hCaptcha says that it runs unseen, and choosing hCaptcha again shows the mode as it was kept', async () => {
  const { client, switchTo } = cardOn();
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'hcaptcha', 'human_check.site_key': 'a-site-key', 'human_check.mode': 'invisible' }));
  show(<HumanCheckCard />, client);
  const mode = async () => screen.findByLabelText(t('admin.signIn.humanCheck.widget'));
  expect(await mode()).toHaveTextContent(t('choiceCatalog.humanCheckWidget.invisible.name'));
  expect(screen.queryByText(t('admin.signIn.humanCheck.unseen'))).not.toBeInTheDocument();
  // reCAPTCHA is chosen: the kept mode is neither shown nor offered, and the card says what the service does.
  switchTo({ 'human_check.provider': 'recaptcha', 'human_check.mode': 'invisible' });
  expect(await screen.findByText(t('admin.signIn.humanCheck.unseen'))).toBeVisible();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.widget'))).not.toBeInTheDocument();
  // hCaptcha is chosen again: its mode is back, as it was kept.
  switchTo({ 'human_check.provider': 'hcaptcha', 'human_check.mode': 'invisible' });
  expect(await mode()).toHaveTextContent(t('choiceCatalog.humanCheckWidget.invisible.name'));
  expect(screen.queryByText(t('admin.signIn.humanCheck.unseen'))).not.toBeInTheDocument();
});

it('shows a stored box mode of an earlier choice neither with reCAPTCHA nor as its own, and offers it to hCaptcha when that is chosen', async () => {
  const { client, switchTo } = cardOn();
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'recaptcha', 'human_check.site_key': 'a-site-key', 'human_check.mode': 'checkbox' }));
  show(<HumanCheckCard />, client);
  expect(await screen.findByText(t('admin.signIn.humanCheck.unseen'))).toBeVisible();
  expect(screen.queryByLabelText(t('admin.signIn.humanCheck.widget'))).not.toBeInTheDocument();
  expect(document.body.textContent).not.toContain(t('choiceCatalog.humanCheckWidget.checkbox.name'));
  switchTo({ 'human_check.provider': 'hcaptcha', 'human_check.mode': 'checkbox' });
  expect(await screen.findByLabelText(t('admin.signIn.humanCheck.widget'))).toHaveTextContent(t('choiceCatalog.humanCheckWidget.checkbox.name'));
  expect(screen.queryByText(t('admin.signIn.humanCheck.unseen'))).not.toBeInTheDocument();
});

it('says which hosts the web edge leaves out where its policy blocks the service, and says only that where it could not be read', async () => {
  calls.settings.mockResolvedValue(settings({ 'human_check.provider': 'hcaptcha', 'human_check.site_key': 'a-site-key' }));
  calls.state.mockResolvedValue({ secret_set: false, secret_changed_at: null, edge: { state: 'blocks', via: 'public', lacking: ['https://hcaptcha.com in script-src', 'https://hcaptcha.com in frame-src', 'https://*.hcaptcha.com in script-src'] } });
  const view = show(<HumanCheckCard />);
  // Each host once, whatever the number of directives it is left out of, and the service by its name.
  expect(await screen.findByText(i18n.t('admin.signIn.humanCheck.edgeBlocks', { service: t('choiceCatalog.humanCheckProvider.hcaptcha.name'), hosts: 'https://hcaptcha.com, https://*.hcaptcha.com' }))).toBeVisible();
  view.unmount();
  calls.state.mockResolvedValue({ secret_set: false, secret_changed_at: null, edge: { state: 'unknown', via: null, lacking: [] } });
  show(<HumanCheckCard />);
  expect(await screen.findByText(t('admin.signIn.humanCheck.edgeUnknown'))).toBeVisible();
  expect(screen.queryByText(/lacks/)).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`says in words and the way on that the service cannot be chosen yet, in ${locale}`, async () => {
    await setLocale(locale);
    expect(t('errors.human_check.edge_blocked')).toContain(locale === 'en' ? 'operator guide' : '运维指南');
    expect(t('auth.humanCheck.unavailable')).toBe(locale === 'en' ? 'The check could not be loaded.' : '无法加载验证。');
  });
}
