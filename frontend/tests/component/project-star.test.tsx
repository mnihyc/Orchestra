/** A compact project star updates the saved view from one act and restores the old list when refused. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import type * as Day100 from '@/api/day100';
import { ProjectStar } from '@/features/project/ProjectStar';
import { SHIPPED_PROJECT_VIEW } from '@/features/project/viewState';
import type { Favorite, ProjectView } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<() => Promise<Favorite[]>>(), star: vi.fn<(id: string, view: ProjectView | null) => Promise<void>>(), remove: vi.fn<(id: string) => Promise<void>>() }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  favoritesQuery: { queryKey: ['favorites'], initialData: (): Favorite[] => [], queryFn: calls.read, staleTime: Infinity }, unstarNode: calls.remove,
}));
vi.mock('@/api/day100', async (original) => ({ ...await original<typeof Day100>(), starProjectView: calls.star }));
const view: ProjectView = { ...SHIPPED_PROJECT_VIEW, view: 'board', tag_ids: ['visible'], with_folders: true };

beforeEach(() => { vi.clearAllMocks(); calls.star.mockResolvedValue(undefined); calls.remove.mockResolvedValue(undefined); calls.read.mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('stars a view, clears only the saved view and removes the star without a follow-up read in %s', async (locale) => {
  await setLocale(locale); const user = userEvent.setup();
  show(<ProjectStar projectId="release" name="Release" view={view} readableTags={['visible']} />);
  const star = screen.getByRole('button', { name: i18n.t('day100.views.starMenu', { name: 'Release' }) });
  expect(star).not.toHaveTextContent(i18n.t('places.star.star'));
  await user.click(star); await user.click(screen.getByRole('menuitem', { name: i18n.t('day100.views.starView') }));
  await waitFor(() => expect(calls.star).toHaveBeenCalledWith('release', view));
  expect(await screen.findByRole('link', { name: i18n.t('day100.views.openStarred') })).toHaveAttribute('href', expect.stringContaining('view=board'));
  expect(star).toHaveAttribute('aria-pressed', 'true');
  await user.click(star); await user.click(screen.getByRole('menuitem', { name: i18n.t('day100.views.starPlace') }));
  await waitFor(() => expect(calls.star).toHaveBeenLastCalledWith('release', null));
  expect(screen.queryByRole('link', { name: i18n.t('day100.views.openStarred') })).not.toBeInTheDocument();
  expect(star).toHaveAttribute('aria-pressed', 'true');
  await user.click(star); await user.click(screen.getByRole('menuitem', { name: i18n.t('day100.views.removeStar') }));
  await waitFor(() => expect(calls.remove).toHaveBeenCalledWith('release'));
  await waitFor(() => expect(star).toHaveAttribute('aria-pressed', 'false'));
  expect(calls.star).toHaveBeenCalledTimes(2); expect(calls.remove).toHaveBeenCalledTimes(1); expect(calls.read).not.toHaveBeenCalled();
});

it('puts back an optimistic star if the only write is refused', async () => {
  calls.star.mockRejectedValue(new Error('the write was refused')); const user = userEvent.setup();
  show(<ProjectStar projectId="release" name="Release" view={view} readableTags={['visible']} />);
  const star = screen.getByRole('button', { name: i18n.t('day100.views.starMenu', { name: 'Release' }) });
  await user.click(star); await user.click(screen.getByRole('menuitem', { name: i18n.t('day100.views.starView') }));
  await waitFor(() => expect(calls.star).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(star).toHaveAttribute('aria-pressed', 'false'));
  expect(screen.queryByRole('link', { name: i18n.t('day100.views.openStarred') })).not.toBeInTheDocument();
  expect(calls.read).not.toHaveBeenCalled();
});
