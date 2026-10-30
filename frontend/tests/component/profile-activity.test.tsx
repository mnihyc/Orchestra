/** Visible calendar counts are accessible without colour and the table includes every UTC day. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ActivityCalendar } from '@/features/profile/Activity';
import type { ProfileActivity } from '@/generated/api-types';
import { show } from './support';

const activity: ProfileActivity = { from: '2026-03-07', to: '2026-03-10', days: [{ day: '2026-03-08', turns: 4 }], total: 4, recent: [] };

describe('visible work by UTC day', () => {
  it('names every square including empty days and exposes identical figures in the table', async () => {
    show(<ActivityCalendar activity={activity} />);
    expect(screen.getByRole('img', { name: '2026-03-08: 4 turns' })).toBeVisible();
    expect(screen.getByRole('img', { name: '2026-03-09: 0 turns' })).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Show as a table' }));
    const table = screen.getByRole('table', { name: 'Work by day' });
    expect(within(table).getAllByRole('row')).toHaveLength(5);
    expect(within(table).getByRole('row', { name: '2026-03-08 4' })).toBeVisible();
  });
});
