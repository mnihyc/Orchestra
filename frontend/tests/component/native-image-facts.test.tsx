/** Recorded image delivery stays in the attachment's existing fact opening. */
import { useState } from 'react';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { FileFactBody, imageDeliveryFragment } from '@/features/files/FileFacts';
import { FactDisclosure } from '@/ui/FactDisclosure';
import type { NativeImageReceipt } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
function RecordedImage({ receipt }: { receipt: NativeImageReceipt | null }) {
  const [open, setOpen] = useState(false);
  return <FactDisclosure groups={[]} open={open} onOpenChange={setOpen} fragment={imageDeliveryFragment(receipt, i18n.t.bind(i18n))}>
    <FileFactBody imageDelivery={receipt} />
  </FactDisclosure>;
}
afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('keeps the recorded path allowance and an unknown old decision separate in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const receipt: NativeImageReceipt = { part: 0, forwarded_image: null, applied_delivery: 'path', allowance_exceeded: true, native_image_bytes: 14,
    resolved_at: '2026-11-15T03:00:00Z', source: { scope: 'default', node: null } };
  const page = show(<RecordedImage receipt={receipt} />);
  expect(screen.queryByText(i18n.t('fileFacts.nativeImageBytes'))).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(await screen.findByText(i18n.t('fileFacts.nativeImageBytes'))).toBeVisible();
  expect(screen.getByText('14 B')).toBeVisible();
  expect(screen.getAllByText(i18n.t('fileFacts.imageByPath')).length).toBeGreaterThan(0);
  page.rerender(<RecordedImage receipt={null} />);
  expect(screen.queryByText('14 B')).toBeNull();
  expect(screen.getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThan(0);
});
