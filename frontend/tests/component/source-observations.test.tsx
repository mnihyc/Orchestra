/** Work observations and the host's observation time use readable words without hiding a failed turn. */
import { screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { HostPortsDeclaration } from '@/features/ports/HostDeclaration';
import { WorkChips } from '@/features/source/Attention';
import { ThreadCard } from '@/features/topic/ThreadCard';
import type { ThreadAttention, TreeNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type { HostPortsReview } from '@/features/ports/presentation';
import { acmeTree } from '../doubles/fixtures';
import { display } from '../doubles/thread-display';
import { show } from './support';

const observed = '2026-10-15T01:05:57.200Z';
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
vi.mock('@/features/navigation/places', async (original) => ({ ...await original<object>(),
  PlaceLink: ({ place, children, className }: { place: TreeNode; children: ReactNode; className?: string }) => <a href={`#${place.id}`} className={className}>{children}</a>,
}));
afterEach(async () => {
  useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

function card(attention: ThreadAttention): TreeNode {
  const thread = acmeTree().find((node) => node.kind === 'thread');
  if (!thread?.thread) throw new Error('the fixture contains a real thread summary');
  return { ...thread, name: 'Repair login', archived: false,
    thread: { ...thread.thread, state: 'idle', display: display(attention.failed ? 'failed' : 'idle'), pending_approvals: 0, last_activity_at: null, attention } };
}
const host: HostPortsReview = {
  current: null, proposal: { address: 'host.example', bind: 'all', ranges: [{ from: 41000, to: 41015 }] },
  allocations: [], revision: 1, acknowledged: null, can_edit: false, reason: null,
  sources: { incoming_address: 'configured address', listen: 'configured listeners', ranges: 'configured ranges' }, as_of: observed,
};
vi.mock('@/api/ports', () => ({
  hostPortsQuery: (id: string) => ({ queryKey: ['observation-host-ports', id], queryFn: () => Promise.resolve(host) }),
  saveHostPorts: vi.fn(),
}));

const directory: ThreadAttention = { uncommitted: null, unpushed: null, failed: false, as_of: observed, stale: true, reason: 'not_a_repository' };

describe('observed work in the thread heading', () => {
  it('shows nothing for a non-Git directory, including in compact rows and on cards', () => {
    const { container } = show(<><WorkChips attention={directory} /><WorkChips attention={directory} compact /><WorkChips attention={directory} context="card" /></>);
    expect(container.querySelector('[data-work-chips]')).not.toBeInTheDocument();
    expect(screen.queryByText('not_a_repository')).not.toBeInTheDocument();
    // A plain directory is normal: no word suggests that the thread needs a repository.
    expect(container).not.toHaveTextContent(/Git|repository/i);
  });
  it('keeps a failed turn visible without borrowing the non-repository observation time', () => {
    const { container } = show(<WorkChips attention={{ ...directory, failed: true }} />);
    expect(screen.getByText('Last turn failed')).toBeVisible();
    expect(container.querySelector('time')).not.toBeInTheDocument();
    expect(screen.queryByText('not_a_repository')).not.toBeInTheDocument();
    expect(container).not.toHaveTextContent(/Git|repository/i);
  });
  it('explains offline Git and retains its formatted observation time', () => {
    const offline: ThreadAttention = { ...directory, reason: 'source_offline' };
    const { container } = show(<WorkChips attention={offline} />);
    expect(screen.getByText('Git status unavailable')).toBeVisible();
    expect(screen.getByText('The environment is offline; these are its last observed Git facts.')).toBeVisible();
    const time = container.querySelector('time');
    expect(time).toHaveAttribute('datetime', observed);
    expect(time).not.toHaveTextContent(observed);
    expect(container).not.toHaveTextContent('source_offline');
  });
});

it('formats the host port proposal time while retaining the exact timestamp for its time element', async () => {
  const hostId = 'host-words';
  const { container } = show(<HostPortsDeclaration hostId={hostId} />);
  await screen.findByText(/Proposal sources/);
  const time = container.querySelector('time');
  expect(time).toHaveAttribute('datetime', observed);
  expect(time).not.toHaveTextContent(observed);
});

for (const [locale, theme] of modes) {
  it(`keeps an environment-wide unknown off a thread card while retaining it in detail (${locale}/${theme})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const attention: ThreadAttention = { ...directory, reason: 'source_offline' };
    const view = show(<ul><ThreadCard thread={card(attention)} /></ul>);
    expect(screen.getByRole('link', { name: 'Repair login' })).toBeVisible();
    expect(screen.queryByText(i18n.t('source.work.unknown'))).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('source.work.stale'))).not.toBeInTheDocument();
    expect(view.container.querySelector('[data-work-chips]')).not.toBeInTheDocument();
    view.unmount();
    show(<WorkChips attention={attention} />);
    expect(screen.getByText(i18n.t('source.work.unknown'))).toBeVisible();
    expect(screen.getByText(i18n.t('source.work.reasons.offline'))).toBeVisible();
  });

  it(`retains this card's observed changes and failed turn without the environment warning (${locale}/${theme})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    show(<ul><ThreadCard thread={card({ ...directory, uncommitted: true, unpushed: true, failed: true, reason: 'source_offline' })} /></ul>);
    expect(screen.getByText(i18n.t('source.work.uncommitted'))).toBeVisible();
    expect(screen.getByText(i18n.t('source.work.unpushed'))).toBeVisible();
    expect(screen.getByText(i18n.t('source.work.failed'))).toBeVisible();
    expect(screen.getAllByText(i18n.t('source.work.stale'))).toHaveLength(1);
    expect(screen.queryByText(i18n.t('source.work.unknown'))).not.toBeInTheDocument();
  });
}
