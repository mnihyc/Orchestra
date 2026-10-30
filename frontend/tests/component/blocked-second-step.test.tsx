/** A held sender's second step is solved under Security, whatever the reader's thread role. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import type { Thread } from '@/generated/api-types';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ Link: ({ to, children, className }: { to: string; children: ReactNode; className?: string }) => <a href={to} className={className}>{children}</a> }));

function draw(manages: boolean, detail: string) {
  const thread = { id: 'thread-1', name: 'Repair login', state: 'blocked', lock: null, blocked: { reason: 'binding_invalid', detail }, binding: null, owner: { display_name: 'Dana' }, access: { permissions: manages ? ['thread.view', 'thread.manage'] : ['thread.view'] } } as unknown as Thread;
  show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><BlockedBanner thread={thread} onProceed={vi.fn()} /></QueryClientProvider>);
}

describe('a thread held for a second step', () => {
  for (const manages of [false, true]) {
    it(`directs ${manages ? 'a manager' : 'a reader'} to Security and keeps the held messages`, () => {
      draw(manages, 'second_step_required');
      expect(screen.getByText(/held until their senders set up the second step/)).toBeVisible();
      expect(screen.getByRole('link')).toHaveAttribute('href', '/settings/security');
      expect(screen.queryByRole('button', { name: /Switch/ })).not.toBeInTheDocument();
      expect(screen.queryByText(/Dana/)).not.toBeInTheDocument();
      expect(screen.queryByText(/arrangement is no longer allowed/)).not.toBeInTheDocument();
    });
  }
  it('still offers a profile change for an invalid arrangement', () => {
    draw(true, 'This arrangement was withdrawn');
    expect(screen.getByRole('button', { name: /Switch/ })).toBeVisible();
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });
});
