/** The admitted delivery receipt survives changing defaults and old inputs never borrow a current budget. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { NativeImageFacts } from '@/features/composer/NativeImageFacts';
import { client } from '@/api/client';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { client.session.setState({ status: 'signed_out', userId: null }); await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('keeps the recorded path allowance and an unknown old decision separate in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const receipt = { native_image_bytes: 14, resolved_at: '2026-11-15T03:00:00Z', source: { scope: 'default' as const, node: null }, images: [{ name: 'diagram.png', bytes: 15, delivery: 'path' as const, reason: 'allowance_exceeded' as const }] };
  const shown = show(<NativeImageFacts receipt={receipt} thread="thread" />);
  expect(screen.queryByText('diagram.png')).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByText('diagram.png')).toBeVisible();
  expect(screen.getByText(i18n.t('composer.imageFacts.allowance'))).toBeVisible();
  expect(screen.getByText(i18n.t('composer.imageFacts.exceeded'))).toBeVisible();
  shown.rerender(<NativeImageFacts thread="thread" />);
  expect(screen.queryByText('diagram.png')).toBeNull();
  expect(screen.getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThan(0);
});
