/** A shortcut names its target as its reader may see it and leads there, says when it does not open, and moves or removes only itself; "Add shortcut…" keeps one in the place chosen. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Shortcuts from '@/api/shortcuts';
import type { PersonalTarget, Shortcut } from '@/generated/api-types';
import { ShortcutLink, ShortcutMenuButton } from '@/features/navigation/shortcuts';
import { useDialogState } from '@/features/navigation/NodeDialogs';
import { AddShortcutDialog } from '@/features/navigation/ShortcutDialog';
import type { PickedPlace, PlacePickerProps } from '@/features/settings/PlacePicker';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({
  keep: vi.fn<(nodeId: string, target: PersonalTarget) => Promise<Shortcut>>(),
  remove: vi.fn<(id: string) => Promise<void>>(),
  move: vi.fn<(id: string, nodeId: string | null) => Promise<Shortcut>>(),
}));
vi.mock('@/api/shortcuts', async (original) => ({ ...await original<typeof Shortcuts>(), keepShortcut: calls.keep, removeShortcut: calls.remove, moveShortcut: calls.move }));
/** The place picker chooses by its own reads; here it is one button that chooses the place Web / Plans. */
const plans: PickedPlace = { id: 'plans', kind: 'topic', color: null, folder: false, path: 'Acme / Web / Plans' };
vi.mock('@/features/settings/PlacePicker', () => ({
  PlacePicker: ({ label, onChange }: PlacePickerProps) => <button type="button" onClick={() => onChange(plans)}>{label}</button>,
}));

const shortcut = (more: Partial<Shortcut> = {}): Shortcut => ({
  id: 'kept',
  node_id: 'web',
  target: { kind: 'document', id: 'plan' },
  position: '1',
  state: 'open',
  item: { name: 'Launch plan', path: [{ id: 'acme', kind: 'org', name: 'Acme', redacted: false }], org_id: 'acme', archived: false, color: null },
  view: null,
  ...more,
});

beforeEach(() => {
  vi.clearAllMocks();
  calls.keep.mockResolvedValue(shortcut({ node_id: 'plans' }));
  calls.remove.mockResolvedValue(undefined);
  calls.move.mockResolvedValue(shortcut());
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('names an open target and leads to it, with the arrow that says it is a shortcut, in %s', async (locale) => {
  await setLocale(locale);
  show(<ShortcutLink shortcut={shortcut()} className="row" />);
  const link = screen.getByRole('link', { name: /Launch plan/ });
  expect(link).toHaveAttribute('href', '/doc/plan');
  expect(link).toHaveAttribute('draggable', 'true');
  expect(link.querySelector('[data-shortcut-icon="document"]')?.children).toHaveLength(2);
});

it.each(['en', 'zh-CN'] as const)('says nothing of a target that does not open, and leads nowhere, in %s', async (locale) => {
  await setLocale(locale);
  const { rerender } = show(<ShortcutLink shortcut={shortcut({ state: 'unavailable', item: null })} className="row" />);
  expect(screen.getByText(i18n.t('shortcuts.state.unavailable'))).toBeVisible();
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(document.body.textContent).not.toContain('Launch plan');
  rerender(<ShortcutLink shortcut={shortcut({ state: 'deleted', item: null })} className="row" />);
  expect(screen.getByText(i18n.t('shortcuts.state.deleted'))).toBeVisible();
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
});

it('moves or removes the shortcut and never its target; one of the person\'s own is unstarred', async () => {
  const user = userEvent.setup();
  const { unmount } = show(<ShortcutMenuButton shortcut={shortcut()} />);
  await user.click(screen.getByRole('button', { name: i18n.t('places.menu.more', { name: 'Launch plan' }) }));
  expect(screen.getAllByRole('menuitem')).toHaveLength(4);
  for (const name of [i18n.t('places.menu.open'), i18n.t('shortcuts.move'), i18n.t('shortcuts.remove')]) expect(screen.getByRole('menuitem', { name })).toBeVisible();
  await user.click(screen.getByRole('menuitem', { name: i18n.t('shortcuts.remove') }));
  await waitFor(() => expect(calls.remove).toHaveBeenCalledWith('kept'));
  expect(await screen.findByText(i18n.t('shortcuts.removed', { name: 'Launch plan' }))).toBeVisible();
  unmount();

  show(<ShortcutMenuButton shortcut={shortcut({ node_id: null })} />);
  await user.click(screen.getByRole('button', { name: i18n.t('places.menu.more', { name: 'Launch plan' }) }));
  await user.click(screen.getByRole('menuitem', { name: i18n.t('shortcuts.move') }));
  expect(useDialogState.getState().current).toMatchObject({ type: 'move-shortcut', name: 'Launch plan' });
  await user.click(screen.getByRole('button', { name: i18n.t('places.menu.more', { name: 'Launch plan' }) }));
  expect(screen.getByRole('menuitem', { name: i18n.t('places.star.unstar') })).toBeVisible();
  expect(screen.queryByRole('menuitem', { name: i18n.t('shortcuts.remove') })).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps a shortcut in the place chosen, says where, and undoes it in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const onClose = vi.fn();
  show(<AddShortcutDialog target={{ kind: 'document', id: 'plan', name: 'Launch plan' }} open onClose={onClose} />);
  expect(screen.getByRole('dialog', { name: i18n.t('shortcuts.addTitle', { name: 'Launch plan' }) })).toBeVisible();
  const add = screen.getByRole('button', { name: i18n.t('shortcuts.addSubmit') });
  expect(add).toBeDisabled();
  await user.click(screen.getByRole('button', { name: i18n.t('shortcuts.where') }));
  await user.click(add);
  await waitFor(() => expect(calls.keep).toHaveBeenCalledWith('plans', { kind: 'document', id: 'plan' }));
  expect(onClose).toHaveBeenCalled();
  expect(await screen.findByText(i18n.t('shortcuts.added', { name: 'Launch plan', place: 'Plans' }))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('places.toast.undo') }));
  await waitFor(() => expect(calls.remove).toHaveBeenCalledWith('kept'));
});
