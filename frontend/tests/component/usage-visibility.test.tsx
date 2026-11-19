/** Usage opens the place's existing access page without a redundant audience sentence/read. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { UsageVisibility } from '@/features/usage/Visibility';
import { openAccess } from '@/features/access/AccessDialog';
import { show } from './support';

vi.mock('@/features/access/AccessDialog', () => ({ openAccess:vi.fn() }));
vi.mock('@/api/nodes', () => ({ nodeQuery:() => ({ queryKey:['visibility-node'],queryFn:() => Promise.resolve({ id:'project',kind:'project',name:'Research' }) }) }));
const reads = vi.hoisted(() => ({ page: vi.fn() }));
vi.mock('@/api/access', () => ({ pageQuery: reads.page }));

it('omits the audience sentence and opens the same access page in one click', async () => {
  show(<UsageVisibility nodeId="project" />);
  const access = await screen.findByRole('button', { name: 'Access to Research' });
  expect(access).toHaveTextContent('Access');
  expect(screen.queryByText(/Seen by|summaries.*reports|owners and administrators/)).not.toBeInTheDocument();
  expect(reads.page).not.toHaveBeenCalled();
  await userEvent.setup().click(access);
  expect(openAccess).toHaveBeenCalledWith({ kind:'project',id:'project',name:'Research' },{ share:false });
});
