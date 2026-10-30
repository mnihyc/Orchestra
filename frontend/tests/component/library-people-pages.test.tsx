/** Reading more recorded people replaces one page without accumulating earlier people in the card. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { LibraryCounts } from '@/features/resources/Library';
import type { LibraryPeopleBody, ResourceFacts } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const reads = vi.hoisted(() => [] as (string | null)[]);
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: (path: string, { body }: { body: LibraryPeopleBody }) => {
    if (path !== '/resources/library/people') throw new Error(`Unexpected read: ${path}`);
    reads.push(body.cursor);
    const first = body.cursor === null ? 1 : 51;
    return Promise.resolve({ response: new Response(null, { status: 200 }), data: {
      items: Array.from({ length: 50 }, (_, index) => userRef(me(first + index, `reader-${first + index}`, `Reader ${first + index}`))),
      hidden: 200, next_cursor: first === 1 ? 'after-50' : null,
    } });
  } } };
});

it('keeps 50 visible people through Next page and Back to beginning with one cursor read per page', async () => {
  const facts: ResourceFacts = {
    subject: { kind: 'resource', object: { kind: 'skill', id: 'skill' } },
    as_of: '2026-10-27T00:00:00Z', dependents: [], dependents_complete: true,
    uses: { kind: 'skill_invocation', coverage: { state: 'partial', since: '2026-10-01T00:00:00Z' },
      all_time: 1_000_000, last_30_days: 1_000, people: 1_000, people_30_days: 1_000,
      last_used_at: '2026-10-27T00:00:00Z' },
  };
  const user = userEvent.setup();
  show(<LibraryCounts facts={facts} full />);
  await user.click(screen.getByRole('button', { name: '1,000 people' }));
  const card = await screen.findByRole('dialog', { name: '1,000 people' });
  expect(await within(card).findByText('Reader 1')).toBeVisible();
  expect(within(card).getAllByRole('listitem')).toHaveLength(50);
  expect(within(card).getByText('and 200 more')).toBeVisible();
  expect(reads).toEqual([null]);
  await user.click(within(card).getByRole('button', { name: 'Next page' }));
  expect(await within(card).findByText('Reader 51')).toBeVisible();
  expect(within(card).queryByText('Reader 1')).not.toBeInTheDocument();
  expect(within(card).getAllByRole('listitem')).toHaveLength(50);
  expect(reads).toEqual([null, 'after-50']);
  await user.click(within(card).getByRole('button', { name: 'Back to beginning' }));
  expect(await within(card).findByText('Reader 1')).toBeVisible();
  expect(within(card).getAllByRole('listitem')).toHaveLength(50);
  expect(reads.every((cursor) => cursor === null || cursor === 'after-50')).toBe(true);
});
