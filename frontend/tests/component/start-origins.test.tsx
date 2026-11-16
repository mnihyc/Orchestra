/** A compact setup says its common source once and keeps every differing choice truthful. */
import { afterEach, expect, it } from 'vitest';
import { cleanup, within } from '@testing-library/react';
import { StartValues } from '@/features/start/ChoiceOrigin';
import { NO_PICKS, resolve, type Resolved } from '@/features/start/choices';
import { i18n, setLocale } from '@/i18n';
import { environment, options } from '../unit/start-fixtures';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
const mode = { suggest: true, respectDefaults: true };
const unset = { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false };

function summary(resolved: Resolved) {
  const name = i18n.t('start.quick.label.topic');
  const view = show(<section aria-label={name}><StartValues resolved={resolved} /></section>);
  return view.getByRole('region', { name });
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names the unusable inherited environment beside the fresh quick-start proposal (${locale})`, async () => {
    await setLocale(locale);
    const failed = { ...environment('spare', { usable: false }), reason: 'environment_failed' };
    const resolved = resolve(options({ environments: [failed], default_environment: { environment: 'spare', from: null, reason: 'environment_failed' } }), NO_PICKS, mode);
    expect(resolved.fresh).toBe(true);
    const view = summary(resolved);
    const value = view.querySelector('[data-start-value="environment"]');
    expect(value).toHaveTextContent(i18n.t('start.quick.inNew'));
    expect(value).toHaveTextContent(locale === 'en' ? 'Env spare unavailable' : 'Env spare 无法使用');
    expect(value).toHaveTextContent(i18n.t('start.reason.environment_failed'));
    expect(view.querySelectorAll('p')).toHaveLength(0);
  });

  it(`keeps an unreadable default unnamed in the quick-start reason (${locale})`, async () => {
    await setLocale(locale);
    const resolved = resolve(options({ environments: [], default_environment: { environment: 'unreadable', from: null, reason: 'environment_not_seen' } }), NO_PICKS, mode);
    const view = summary(resolved);
    const value = view.querySelector('[data-start-value="environment"]');
    expect(value).toHaveTextContent(locale === 'en' ? 'Default environment unavailable' : '默认环境无法使用');
    expect(value).toHaveTextContent(i18n.t('start.reason.environment_not_seen'));
    expect(value).not.toHaveTextContent('environment_not_seen');
  });

  it(`says Suggested once for the whole automatic setup without separators (${locale})`, async () => {
    await setLocale(locale);
    const resolved = resolve(options({ defaults: unset, environments: [environment('ranked')] }), NO_PICKS, mode);
    const view = summary(resolved);
    const list = within(view).getByRole('list');
    const mark = within(view).getByText(i18n.t('startImplicit.origin.suggested'), { exact: true });
    expect(list).not.toContainElement(mark);
    expect(within(list).getAllByRole('listitem')).toHaveLength(6);
    expect(within(list).queryByText(i18n.t('startImplicit.origin.suggested'), { exact: true })).toBeNull();
    expect(within(view).queryByText(i18n.t('startFlow.separator').trim(), { exact: true })).toBeNull();
  });

  it(`marks only choices that differ from the common configured source (${locale})`, async () => {
    await setLocale(locale);
    const resolved = resolve(options({ defaults: { ...unset, provider_id: 'p1', agent_id: 'a1', model_name: 'm1', effort: 'low' } }), NO_PICKS, mode);
    const view = summary(resolved);
    const list = within(view).getByRole('list');
    const common = within(view).getByText(i18n.t('start.topicDefault'), { exact: true });
    expect(list).not.toContainElement(common);
    for (const field of ['agent', 'provider', 'model', 'effort']) {
      const value = view.querySelector(`[data-start-value="${field}"]`);
      expect(value).not.toHaveTextContent(i18n.t('start.topicDefault'));
    }
    const environmentValue = view.querySelector('[data-start-value="environment"]');
    expect(environmentValue).toHaveTextContent(i18n.t('startImplicit.origin.shared_default'));
    const arrangement = view.querySelector('[data-start-value="arrangement"]');
    expect(arrangement).toHaveTextContent(i18n.t('startImplicit.origin.suggested'));
  });

  it(`distinguishes a manual override from the remaining suggested setup (${locale})`, async () => {
    await setLocale(locale);
    const resolved = resolve(options({ defaults: unset, environments: [environment('ranked')] }), { ...NO_PICKS, model: 'm1' }, mode);
    const view = summary(resolved);
    expect(within(view).getAllByText(i18n.t('startImplicit.origin.suggested'), { exact: true })).toHaveLength(1);
    expect(within(view).getAllByText(i18n.t('startImplicit.origin.manual'), { exact: true })).toHaveLength(1);
    expect(view.querySelector('[data-start-value="model"]')).toHaveTextContent(i18n.t('startImplicit.origin.manual'));
  });

  it(`adds no automatic origin to an entirely explicit setup (${locale})`, async () => {
    await setLocale(locale);
    const resolved = resolve(options(), { provider: 'p1', agent: 'a1', model: 'm1', effort: 'low', environment: 'e1', arrangement: 'separated' }, mode);
    const view = summary(resolved);
    expect(within(view).getAllByRole('listitem')).toHaveLength(6);
    for (const key of ['suggested', 'manual', 'last_used', 'shared_default'] as const) {
      expect(within(view).queryByText(i18n.t(`startImplicit.origin.${key}`), { exact: true })).toBeNull();
    }
    expect(within(view).queryByText(i18n.t('start.topicDefault'), { exact: true })).toBeNull();
  });
}
