/** Saved profile config names stay readable without leaking their values or guessing program semantics. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { ProfileConfigKeys, profileConfigKeys } from '@/features/resources/agents/ProfileConfigKeys';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const config = { service_tier: 'private-fast-value', model_context_window: 64_000,
  model_reasoning_effort: 'low', model_reasoning_summary: 'concise', web_search: 'indexed',
  features: { local_option: false }, extra_dirs: ['private-path'], 'literal.dot': 'private-literal' };
afterEach(async () => { await setLocale('en'); });

it('distinguishes nested keys, literal dotted keys, empty tables and absent config', () => {
  expect(profileConfigKeys({ config: { nested: { key: true }, 'nested.key': false, empty: {}, values: [1, 2], omitted: null }, env: { TOKEN: 'private' } }))
    .toEqual(['"nested.key"', 'empty', 'nested.key', 'omitted', 'values']);
  expect(profileConfigKeys(null)).toEqual([]);
  expect(profileConfigKeys({ config: null })).toEqual([]);
  expect(profileConfigKeys({ config: [] })).toEqual([]);
});

it('walks deep configuration names without recursive calls or copying every parent path', () => {
  let nested: Record<string, unknown> = { leaf: false };
  // Deliberately beyond an ordinary call stack; this is a source fixture, not a product limit.
  for (let level = 0; level < 20_000; level += 1) nested = { child: nested };
  const keys = profileConfigKeys({ config: nested });
  expect(keys).toHaveLength(1);
  expect(keys[0]?.endsWith('.leaf')).toBe(true);
});

for (const locale of ['en', 'zh-CN'] as const) it(`opens the complete saved key list with the existing preview in ${locale}`, async () => {
  await setLocale(locale);
  const view = show(<ul><ProfileConfigKeys profile={{ config, env: { TOKEN: 'private-env' } }} /></ul>);
  const list = screen.getByRole('list', { name: locale === 'en' ? 'Configuration keys' : '配置键' });
  expect(within(list).queryByText('web_search', { exact: true })).not.toBeInTheDocument();
  const more = screen.getByRole('button', { name: i18n.t('ui.preview.showAll') });
  more.focus();
  await userEvent.keyboard('{Enter}');
  expect(within(list).getByText('web_search', { exact: true })).toBeVisible();
  expect(within(list).getByText('features.local_option', { exact: true })).toBeVisible();
  expect(within(list).getByText('"literal.dot"', { exact: true })).toBeVisible();
  expect(list).not.toHaveTextContent('private-fast-value');
  expect(list).not.toHaveTextContent('private-env');
  expect(list).not.toHaveTextContent('private-path');
  view.rerender(<ul><ProfileConfigKeys profile={{ config: { model_reasoning_effort: 'high' } }} /></ul>);
  expect(screen.getByRole('list', { name: i18n.t('agents.profile.configKeys') })).toHaveTextContent('model_reasoning_effort');
  expect(screen.queryByText('web_search', { exact: true })).not.toBeInTheDocument();
});
