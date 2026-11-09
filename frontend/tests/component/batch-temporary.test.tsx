/** The batch/recovery destination makes one shared environment and gives it idle-based temporary consent. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { BatchDestination } from '@/features/change/BatchDestination';
import type { ForkEnvironment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof import('@/api/environments')>(), environmentsQuery: () => ({ queryKey: ['temporary-batch-environments'], queryFn: () => Promise.resolve([]) }) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof import('@/api/resources')>(), hostsQuery: () => ({ queryKey: ['temporary-batch-hosts'], queryFn: () => Promise.resolve([]) }) }));
function Placement({ allowSame }: { allowSame: boolean }) {
  const [value, setValue] = useState<ForkEnvironment | undefined>();
  return <><BatchDestination value={value} onChange={setValue} allowSame={allowSame} /><output aria-label="Batch environment">{JSON.stringify(value)}</output></>;
}
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const allowSame of [true, false]) it('offers idle cleanup for a shared new destination ('+locale+', keep='+String(allowSame)+')', async () => {
  await setLocale(locale); const user = userEvent.setup(); show(<Placement allowSame={allowSame} />);
  expect(screen.queryByRole('switch', { name: i18n.t('temporary.name') })).not.toBeInTheDocument();
  await user.click(screen.getByRole('combobox', { name: i18n.t('loss.destination') }));
  await user.click(await screen.findByRole('option', { name: i18n.t('loss.newEnvironment') }));
  const temporary = screen.getByRole('switch', { name: i18n.t('temporary.name') });
  expect(temporary).toHaveAccessibleDescription(i18n.t('temporary.idleDescription'));
  await user.click(temporary);
  expect(screen.getByLabelText('Batch environment')).toHaveTextContent('{"new":{"host_id":null,"config":{"env.lifetime":"temporary"}}}');
  await user.unhover(temporary); await user.hover(temporary);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('temporary.idleConsent'));
  await user.click(temporary);
  expect(screen.getByLabelText('Batch environment')).toHaveTextContent('{"new":{"host_id":null}}');
});
