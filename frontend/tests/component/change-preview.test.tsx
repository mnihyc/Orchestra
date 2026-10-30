/** What a change is said to reach before it is confirmed: how many pages follow the rows, in how many orgs where it is the system's own defaults, and who gains and who loses, with the names behind the counts for whoever opens them. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { Account } from '@/features/access/ChangePreview';
import type { PagePreview, PreviewPerson, UserRef } from '@/generated/api-types';

import { show } from './support';

function person(name: string, n: number): UserRef {
  return {
    id: `00000000-0000-4000-8000-${n.toString().padStart(12, '0')}`,
    kind: 'human',
    username: name.split(' ')[0]?.toLowerCase() ?? name,
    display_name: name,
    avatar: { text: name.slice(0, 1), color: n },
  };
}

const losing = (name: string, n: number, permissions: string[]): PreviewPerson => ({ user: person(name, n), permissions, pages: 1 });

function draw(preview: PagePreview) {
  // The names of permissions come from the catalogue the page was drawn from; without it a permission is named by its key.
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, enabled: false } } });
  return show(
    <QueryClientProvider client={queryClient}>
      <Account preview={preview} />
    </QueryClientProvider>,
  );
}

describe('what a change reaches, as the server says it', () => {
  it('counts the pages that follow the rows, and the orgs they are in where the server says how many', () => {
    draw({ pages: 12, orgs: 3, gaining: [], losing: [], more: 0 });

    expect(screen.getByText(/12 pages follow these rows\./)).toHaveTextContent('They are in 3 orgs.');
    expect(screen.getByText('Nobody gains or loses anything.')).toBeInTheDocument();
  });

  it('does not say how many orgs where the server does not', () => {
    draw({ pages: 12, orgs: null, gaining: [], losing: [], more: 0 });

    expect(screen.getByText(/12 pages follow these rows\./)).not.toHaveTextContent('orgs');
  });

  it('keeps the names behind the counts until they are asked for, with what each gains or loses', async () => {
    const user = userEvent.setup();
    draw({ pages: 1, orgs: null, gaining: [], losing: [losing('Bea Reader', 5, ['doc.view', 'chat.view']), losing('Cy Writer', 6, ['doc.edit'])], more: 4 });

    const summary = screen.getByText('2 people lose access');
    expect(screen.getByText('Bea Reader')).not.toBeVisible();
    await user.click(summary);
    expect(screen.getByText('Bea Reader')).toBeVisible();
    expect(screen.getByText('Cy Writer')).toBeVisible();
    expect(screen.getByText('and 4 more people')).toBeInTheDocument();
    expect(screen.queryByText('Nobody gains or loses anything.')).not.toBeInTheDocument();
  });
});
