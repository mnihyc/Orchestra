/** Supplied login/notifier declarations keep defaults, descriptions and field drafts until a deliberate choice. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import type * as Login from '@/api/provider-plan';
import { ProviderLogin } from '@/features/resources/providers/Login';
import { NotifierDialog } from '@/features/settings/NotifierForm';
import type { Field, LoginMethod, LoginState, Notifier, ProviderSetup } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { id, me, userRef } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ setup: vi.fn<() => Promise<ProviderSetup>>(), progress: vi.fn<() => Promise<LoginState>>(), start: vi.fn<typeof Login.startProviderLogin>(), submit: vi.fn() }));
const HOME_ID = id(71, 1), PROVIDER = id(71, 2), PROFILE = id(71, 3);
const field = (name: string, label: string, type: Field['type'], value: Field['default'] = null): Field => ({ name, label, type, required: true, default: value, choices: null, help: `Declared help for ${label}.`, max_bytes: null });
const methods: LoginMethod[] = Array.from({ length: 13 }, (_, index) => ({ id: `method-${index + 1}`, title: `Declared login ${index + 1}`,
  summary: `Use account route ${index + 1}.`, kind: 'interactive', yields: 'login', recommended: index === 12, lifetime_secs: null,
  requires: `Account prerequisite ${index + 1}`, fields: [field('region', `Region ${index + 1}`, 'text', `zone-${index + 1}`)] }));
const profile = { id: PROFILE, name: 'Declared adapter', address: 'workshop.adapter', harness_kind: 'metadata-adapter', harness: { kind: 'metadata-adapter', name: 'Declared adapter' }, machine: null, disabled: false };
const setup: ProviderSetup = { subscriptions: [{ agent: profile, harness: { kind: profile.harness_kind, name: profile.name }, methods, options_schema: {}, arrangements: [], template: { id: id(71, 4), name: 'Provider runtime' } }],
  subscriptions_withheld: [], subscriptions_unshared: [], machines: [], key_machines: [], protocols: [], presets: [], api_arrangements: [], runtime_templates: [], sources: [], http_allowed: false };
const idle: LoginState = { state: 'idle', method: null, asks: null, shows: [], expires_at: null, error: null };
const provider = planProvider({ id: PROVIDER, owner: userRef(me(1, 'ada', 'Ada Lovelace')), home: { id: HOME_ID, kind: 'org', name: 'Workshop', redacted: false }, org_id: HOME_ID, agent: profile, harness_kind: profile.harness_kind,
  runtime_support: { lifecycle: 'attach', after_up: false, commands_after_start: { supported: true, reason: null }, sign_in: { allowed: true, home_owned: true, policy_allowed: false } } });
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), providerSetupQuery: (home: string, provider: string) => ({ queryKey: ['metadata-login-setup', home, provider], queryFn: calls.setup }) }));
vi.mock('@/api/provider-plan', async (load) => ({ ...await load<typeof Login>(), startProviderLogin: calls.start,
  loginStateQuery: (provider: string) => ({ queryKey: ['metadata-login-progress', provider], queryFn: calls.progress }) }));

beforeEach(async () => { await setLocale('en'); vi.clearAllMocks(); calls.setup.mockResolvedValue(setup); calls.progress.mockResolvedValue(idle); calls.start.mockResolvedValue(idle); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps the late recommended login and edited fields through search, then starts only the explicitly selected native method in %s', async (locale) => {
  await setLocale(locale); const user = userEvent.setup(); show(<ProviderLogin provider={provider} />);
  const picker = await screen.findByRole('combobox', { name: i18n.t('providerLogin.method') });
  expect(picker).toHaveValue('Declared login 13');
  expect(picker).toHaveAccessibleDescription('Use account route 13.');
  const original = screen.getByRole('textbox', { name: 'Region 13' });
  await user.clear(original); await user.type(original, 'edited-zone');
  await user.click(picker); expect(screen.getAllByRole('option')).toHaveLength(13);
  await user.type(picker, 'method-12');
  expect(screen.getByText('Use account route 12.', { exact: true })).toBeVisible();
  expect(original).toHaveValue('edited-zone'); expect(calls.start).not.toHaveBeenCalled();
  expect(calls.setup).toHaveBeenCalledOnce(); expect(calls.progress).toHaveBeenCalledOnce();
  await user.keyboard('{Escape}'); expect(picker).toHaveValue('Declared login 13'); expect(original).toHaveValue('edited-zone');
  await user.click(picker); await user.type(picker, 'method-12'); await user.keyboard('{Enter}');
  expect(picker).toHaveValue('Declared login 12'); expect(picker).toHaveAccessibleDescription('Use account route 12.'); expect(screen.getByRole('textbox', { name: 'Region 12' })).toHaveValue('zone-12');
  expect(screen.getByText('Account prerequisite 12', { exact: true })).toBeVisible(); expect(calls.start).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.continue') }));
  await waitFor(() => expect(calls.start).toHaveBeenCalledExactlyOnceWith(PROVIDER, 'method-12', { region: 'zone-12' }));
  expect(calls.setup).toHaveBeenCalledOnce();
});

it('searches thirteen caller-eligible notifiers without resetting secrets until selection, then submits only the selected declaration fields', async () => {
  const notifiers: Notifier[] = Array.from({ length: 13 }, (_, index) => ({ name: `channel-${index + 1}`, title: `Delivery method ${index + 1}`, summary: `Deliver through route ${index + 1}.`,
    links: false, single: false, owners: ['org'], targets: ['person'], templates: { single: { subject: null, body: '{{title}}' }, digest: { subject: null, body: '{{count}}' } },
    fields: [field('endpoint', 'Delivery endpoint', 'text', `https://delivery.example.test/${index + 1}`), field('token', 'Delivery token', 'secret')] }));
  const user = userEvent.setup(); show(<NotifierDialog title="Add delivery" notifiers={notifiers} submitLabel="Create delivery" pending={false} error={null} onSubmit={calls.submit} onClose={vi.fn()} />);
  const picker = screen.getByRole('combobox', { name: i18n.t('settings.channels.kind') });
  await user.type(screen.getByRole('textbox', { name: i18n.t('settings.channels.name') }), 'My channel');
  // Leaving the dialog's autofocused locator reveals its committed value.
  expect(picker).toHaveValue('Delivery method 1');
  await user.type(screen.getByLabelText('Delivery token'), 'first-kind-secret');
  await user.click(picker); expect(screen.getAllByRole('option')).toHaveLength(13);
  await user.type(picker, 'channel-13'); expect(screen.getByText('Deliver through route 13.', { exact: true })).toBeVisible();
  expect(screen.getByLabelText('Delivery token')).toHaveValue('first-kind-secret'); expect(calls.submit).not.toHaveBeenCalled();
  await user.keyboard('{Escape}'); expect(picker).toHaveValue('Delivery method 1');
  await user.click(picker); await user.type(picker, 'channel-13'); await user.keyboard('{Enter}');
  expect(screen.getByLabelText('Delivery token')).toHaveValue('');
  expect(screen.getByRole('textbox', { name: 'Delivery endpoint' })).toHaveValue('https://delivery.example.test/13');
  expect(screen.getByRole('textbox', { name: i18n.t('settings.channels.name') })).toHaveValue('My channel');
  expect(calls.submit).not.toHaveBeenCalled();
  await user.type(screen.getByLabelText('Delivery token'), 'selected-kind-secret');
  await user.click(screen.getByRole('button', { name: 'Create delivery' }));
  expect(calls.submit).toHaveBeenCalledExactlyOnceWith({ kind: 'channel-13', name: 'My channel', config: { endpoint: 'https://delivery.example.test/13' }, secrets: { token: 'selected-kind-secret' } });
});
