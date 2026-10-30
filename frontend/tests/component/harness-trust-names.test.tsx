/** Workspace trust uses the reported program name and retains the original policy identity. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { setConfigTrust } from '@/api/trust';
import { HarnessTrustSection } from '@/features/resources/environments/HarnessTrust';
import { i18n, setLocale } from '@/i18n';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@/api/trust', () => ({ setConfigTrust: vi.fn(() => Promise.resolve({})) }));
afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`shows the declared name of a third program for workspace trust (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Research assistant' : '研究助理';
  const person = { id: 'person', kind: 'human' as const, username: 'reviewer', display_name: 'Reviewer', avatar: { text: 'R', color: 1 } };
  const entry = { harness: 'research-agent', program: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } },
    answer: 'trust' as const, path: '/work/config', by: person, at: '2026-10-28T12:00:00Z' };
  const environment = { ...detailEnvironment, config_trust: [entry], access: { ...detailEnvironment.access, permissions: ['environment.manage'] } };
  show(<HarnessTrustSection environment={environment} />);
  const row = screen.getByRole('listitem');
  expect(within(row).getByText(name)).toBeVisible();
  expect(within(row).queryByText('research-agent')).not.toBeInTheDocument();
  expect(within(row).getByText('Reviewer').closest('button')).not.toBeNull();
  await userEvent.click(within(row).getByRole('button', { name: i18n.t('environments.configTrust.keepOut') }));
  await waitFor(() => expect(setConfigTrust).toHaveBeenCalledExactlyOnceWith(environment.id, 'research-agent', 'keep_out'));
});
