/** Notify previews use the same people list as an avatar stack and expose only actual authorized recipients. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { DetailCard } from '@/ui/DetailCard';
import { UserChip } from '@/ui/UserChip';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { i18n } from '@/i18n';
import { show } from './support';
const preview = vi.hoisted(() => vi.fn());
vi.mock('@/api/loss', () => ({ moreOperationRecipients: preview }));
vi.mock('@/features/profile/CardDetails', () => ({ CardDetails: ({ username }: { username: string }) => <p>Profile: {username}</p> }));

const visible = { id: 'person', username: 'operator', display_name: 'Operator', kind: 'human' as const, avatar: { text: 'O', color: 2 } };
it('opens actual recipients on hover, focus and tap, redacts hidden names, and keeps notification consent separate', async () => {
  preview.mockResolvedValue({ total: 2, hidden: 1, people: { items: [visible], next_cursor: null } });
  const changed = vi.fn();
  const user = userEvent.setup();
  show(<NotifyPeople plan={{ people: 2, default_on: true, preview_id: 'reviewed' }} value={undefined} onChange={changed} />);
  const label = i18n.t('operation.notify', { count: 2, number: '2' });
  const trigger = screen.getByRole('button', { name: label });
  const checkbox = screen.getByRole('checkbox', { name: label });
  expect(checkbox).toBeChecked();
  await user.hover(trigger);
  const person = await screen.findByRole('button', { name: 'Operator' });
  expect(person).toBeVisible();
  expect(screen.getByText(i18n.t('ui.avatarStack.andMore', { count: 1 }))).toBeVisible();
  expect(preview).toHaveBeenCalledWith('reviewed', undefined, expect.any(AbortSignal));
  expect(changed).not.toHaveBeenCalled();
  await user.unhover(trigger);
  await waitFor(() => expect(person).not.toBeInTheDocument());
  trigger.focus();
  expect(await screen.findByRole('button', { name: 'Operator' })).toBeVisible();
  await user.click(trigger);
  expect(within(screen.getByRole('dialog', { name: i18n.t('ui.avatarStack.list') })).getByRole('button', { name: 'Operator' })).toBeVisible();
  await user.keyboard('{Escape}');
  await user.click(checkbox);
  expect(changed).toHaveBeenCalledWith(false);
});

it('uses the actual current recipient count after their notification choices changed', async () => {
  preview.mockResolvedValue({ total: 1, hidden: 0, people: { items: [visible], next_cursor: null } });
  const user = userEvent.setup();
  show(<NotifyPeople plan={{ people: 2, default_on: false, preview_id: 'changed-choices' }} value={undefined} onChange={vi.fn()} />);
  await user.hover(screen.getByRole('button', { name: i18n.t('operation.notify', { count: 2, number: '2' }) }));
  expect(await screen.findByRole('button', { name: i18n.t('operation.notify', { count: 1, number: '1' }) })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Operator' })).toBeVisible();
  expect(screen.queryByText(i18n.t('ui.avatarStack.andMore', { count: 1 }))).not.toBeInTheDocument();
});

it('keeps the shared people list present while its UserChip opens a PersonCard', async () => {
  preview.mockResolvedValue({ total: 1, hidden: 0, people: { items: [visible], next_cursor: null } });
  const changed = vi.fn();
  const user = userEvent.setup();
  show(<NotifyPeople plan={{ people: 1, default_on: false, preview_id: 'nested-card' }} value={undefined} onChange={changed} />);
  const trigger = screen.getByRole('button', { name: i18n.t('operation.notify', { count: 1, number: '1' }) });
  await user.hover(trigger);
  await user.click(await screen.findByRole('button', { name: 'Operator' }));
  expect(await screen.findByRole('dialog', { name: 'Operator' })).toHaveTextContent('Profile: operator');
  expect(screen.getByRole('dialog', { name: i18n.t('ui.avatarStack.list') })).toBeVisible();
  expect(changed).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Operator' })).not.toBeInTheDocument();
  expect(screen.getByRole('dialog', { name: i18n.t('ui.avatarStack.list') })).toBeVisible();
});

it('offers no notification consent when all reviewed people have since disabled the kind', async () => {
  preview.mockResolvedValue({ total: 0, hidden: 0, people: { items: [], next_cursor: null } });
  const changed = vi.fn();
  const user = userEvent.setup();
  show(<NotifyPeople plan={{ people: 2, default_on: false, preview_id: 'nobody-receives' }} value={undefined} onChange={changed} />);
  await user.hover(screen.getByRole('button', { name: i18n.t('operation.notify', { count: 2, number: '2' }) }));
  const label = i18n.t('operation.notify', { count: 0, number: '0' });
  expect(await screen.findByRole('button', { name: label })).toBeDisabled();
  expect(screen.getByRole('checkbox', { name: label })).toBeDisabled();
  expect(screen.getByRole('checkbox', { name: label })).not.toBeChecked();
  expect(changed).not.toHaveBeenCalled();
});

it('keeps a shared resource inspection present while its owner opens the same PersonCard', async () => {
  const user = userEvent.setup();
  show(<DetailCard label="Resource owner" content={() => <section><h3>Owner details</h3><UserChip user={visible} /></section>}>Resource owner</DetailCard>);
  screen.getByRole('button', { name: 'Resource owner' }).focus();
  expect(await screen.findByText('Owner details')).toBeVisible();
  await user.click(await screen.findByRole('button', { name: 'Operator' }));
  expect(await screen.findByRole('dialog', { name: 'Operator' })).toHaveTextContent('Profile: operator');
  expect(screen.getByText('Owner details')).toBeVisible();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Operator' })).not.toBeInTheDocument();
  expect(screen.getByText('Owner details')).toBeVisible();
});
