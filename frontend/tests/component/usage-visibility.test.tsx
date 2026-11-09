/** Usage opens the place's existing access page without a redundant audience sentence/read. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { UsageVisibility } from '@/features/usage/Visibility';
import { openAccess } from '@/features/access/AccessDialog';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

vi.mock('@/features/access/AccessDialog', () => ({ openAccess:vi.fn() }));
const read = vi.hoisted(() => vi.fn<() => Promise<{ id: string; kind: 'project'; name: string }>>());
vi.mock('@/api/nodes', () => ({ nodeQuery:() => ({ queryKey:['visibility-node'],queryFn:read,retry:false }) }));
const reads = vi.hoisted(() => ({ page: vi.fn() }));
vi.mock('@/api/access', () => ({ pageQuery: reads.page }));
beforeEach(() => { vi.clearAllMocks(); read.mockReset().mockResolvedValue({ id: 'project', kind: 'project', name: 'Research' }); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps the Usage access entrance while its ordinary target read fails and retries in %s', async (locale) => {
  await setLocale(locale); read.mockRejectedValueOnce(new NetworkError('offline'));
  const user = userEvent.setup();
  show(<UsageVisibility nodeId="project" />);
  await screen.findByRole('alert');
  const unavailable = screen.getByRole('button', { name: i18n.t('usageVisibility.access') });
  expect(unavailable).toHaveAttribute('aria-disabled', 'true');
  await user.click(unavailable); expect(openAccess).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await user.click(await screen.findByRole('button', { name: i18n.t('usageVisibility.open', { place: 'Research' }) }));
  expect(openAccess).toHaveBeenCalledExactlyOnceWith({ kind: 'project', id: 'project', name: 'Research' }, { share: false });
});

it('omits the audience sentence and opens the same access page in one click', async () => {
  show(<UsageVisibility nodeId="project" />);
  const access = await screen.findByRole('button', { name: 'Access to Research' });
  expect(access).toHaveTextContent('Access');
  expect(screen.queryByText(/Seen by|summaries.*reports|owners and administrators/)).not.toBeInTheDocument();
  expect(reads.page).not.toHaveBeenCalled();
  await userEvent.setup().click(access);
  expect(openAccess).toHaveBeenCalledWith({ kind:'project',id:'project',name:'Research' },{ share:false });
});
