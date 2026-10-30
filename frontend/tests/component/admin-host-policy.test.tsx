/** The same observed native settings update inherited host policy without another host read. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';
import { Hosts } from '@/features/admin/Hosts';
import type { ResolvedSetting } from '@/generated/api-types';
import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { detailHost } from './resource-detail-fixtures';

const reads = vi.hoisted(() => ({ hosts: vi.fn(), settings: vi.fn() }));
vi.mock('@/api/admin', () => ({ settingsKey: ['admin', 'settings'], hostsQuery: { queryKey: ['admin', 'hosts'], queryFn: reads.hosts }, systemSettingsQuery: { queryKey: ['admin', 'settings'], queryFn: reads.settings } }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="/">{children}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: () => <span>Keeper</span> }));
const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
afterEach(() => { cache.clear(); vi.clearAllMocks(); });
const setting = (key: string, value: unknown): ResolvedSetting => ({ key, value, writable: false, own: null, locked_by: null, parts: [], source: { scope: 'default', node: null } });

it('follows the accepted system value and threshold while retaining the host’s own override and quiet readers', async () => {
  const shared = { ...detailHost, id: 'shared', name: 'system-following', when_full: { own: false, value: 'warn' as const }, capacity: { ...detailHost.capacity, memory_mb: 100 }, declared: { ...detailHost.declared, memory_mb: 94 } };
  const own = { ...shared, id: 'own', name: 'own-warning', when_full: { own: true, value: 'warn' as const } };
  reads.hosts.mockResolvedValue([shared, own]); reads.settings.mockResolvedValue([setting('hosts.when_full', 'warn'), setting('hosts.full_percent', 90)]);
  render(<QueryClientProvider client={cache}><TooltipProvider><Toaster><Hosts /></Toaster></TooltipProvider></QueryClientProvider>);
  const sharedRow = await screen.findByRole('row', { name: /system-following/ }), ownRow = screen.getByRole('row', { name: /own-warning/ });
  expect(sharedRow).toHaveTextContent('Warns, and lets the host answer'); expect(ownRow).toHaveTextContent('(its own choice)');
  const accept = async (key: string, value: unknown, method: 'PUT' | 'DELETE' = 'PUT') => {
    const found = findAct(method, `/admin/settings/${key}`, { answer: setting(key, value) });
    if (found === null) throw new Error('The system setting act is unregistered');
    await act(async () => { await applyChanges(found.changes, cache); });
  };
  await accept('hosts.when_full', 'block'); await waitFor(() => expect(sharedRow).toHaveTextContent('Takes nothing more'));
  expect(sharedRow).not.toHaveTextContent('(its own choice)'); expect(ownRow).toHaveTextContent('Warns, and lets the host answer');
  await accept('hosts.full_percent', 99); await waitFor(() => expect(sharedRow.querySelector('[data-full]')).toBeNull());
  await accept('hosts.full_percent', 90, 'DELETE'); await waitFor(() => expect(sharedRow).toHaveTextContent('Full'));
  await accept('hosts.when_full', 'warn', 'DELETE'); await waitFor(() => expect(sharedRow).toHaveTextContent('Warns, and lets the host answer'));
  expect(reads.hosts).toHaveBeenCalledOnce(); expect(reads.settings).toHaveBeenCalledOnce();
  expect(cache.getQueryData(['admin', 'hosts'])).toEqual([shared, own]);
});
