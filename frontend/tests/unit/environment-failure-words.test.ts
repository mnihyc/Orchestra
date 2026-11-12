/** Image failures retain their cause and tell a person how to retry, including older stored diagnostics. */
import { afterEach, expect, it } from 'vitest';
import { environmentStatusDetail } from '@/features/resources/environments/words';
import { capabilityConsequence } from '@/features/resources/capabilityWords';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('gives an action and the image cause once in %s', async (locale) => {
  await setLocale(locale);
  for (const detail of ['image: image: pull access denied', 'image: pull access denied']) {
    const words = environmentStatusDetail({ status: 'failed', status_detail: detail }, i18n.t);
    expect(words).toBe(i18n.t('environments.overview.imageFailed', { reason: 'pull access denied' }));
    expect(words).not.toContain('image:');
    expect(words).toContain('pull access denied');
  }
  expect(environmentStatusDetail({ status: 'failed', status_detail: 'the host is offline' }, i18n.t)).toBe('the host is offline');
  expect(environmentStatusDetail({ status: 'running', status_detail: null }, i18n.t)).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('describes host capacity instead of a registry problem in %s', async (locale) => {
  await setLocale(locale);
  for (const detail of ['image: no capacity: no space left on device', 'image: image: no capacity: no space left on device']) {
    const words = environmentStatusDetail({ status: 'failed', status_detail: detail }, i18n.t);
    expect(words).toBe(locale === 'en'
      ? 'The host lacks capacity (no space left on device). Free the affected resources and retry.'
      : '主机资源不足（no space left on device）。请释放对应资源后重试。');
  }
});

it('translates shipped live-application guidance and keeps an authored consequence', async () => {
  const shipped = i18n.getFixedT('en')('approvals.capability.liveEffect');
  await setLocale('zh-CN');
  expect(capabilityConsequence(shipped)).toBe(i18n.t('approvals.capability.liveEffect'));
  expect(capabilityConsequence('Ask the operations team before using this device.')).toBe('Ask the operations team before using this device.');
});

it.each(['en', 'zh-CN'] as const)('keeps gateway-only guidance truthful and localized in %s', async (locale) => {
  const gateway = 'This capability becomes available without stopping anything. Existing processes and all files are kept.';
  expect(i18n.getFixedT('en')('approvals.capability.gatewayEffect')).toBe(gateway);
  await setLocale(locale);
  expect(capabilityConsequence(gateway)).toBe(locale === 'en'
    ? gateway
    : '此能力可立即使用，无需停止任何工作。现有进程和所有文件均会保留。');
  const authored = `${gateway} Ask the operations team first.`;
  expect(capabilityConsequence(authored)).toBe(authored);
});
