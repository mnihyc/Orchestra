/** The finder follows the org tree of the place in view while it is open, so that a screen without the left bar still offers that place's acts. */
import { renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { useFinderTreeFeed } from '@/features/finder/shellCommands';
import { useFinder } from '@/features/finder/store';

const followed = vi.hoisted(() => vi.fn());
vi.mock('@/realtime/live', async (original) => ({ ...await original<object>(), useOrgTreeFeed: (org: string | null) => { followed(org); } }));
afterEach(() => { useFinder.setState({ open: false }); followed.mockClear(); });

describe('the tree behind the finder', () => {
  it('is not followed while the finder is shut', () => {
    renderHook(() => useFinderTreeFeed('org-1'));
    expect(followed).toHaveBeenLastCalledWith(null);
  });

  it('is followed while the finder is open and let go when it shuts', () => {
    useFinder.setState({ open: true });
    const view = renderHook(() => useFinderTreeFeed('org-1'));
    expect(followed).toHaveBeenLastCalledWith('org-1');
    useFinder.setState({ open: false });
    view.rerender();
    expect(followed).toHaveBeenLastCalledWith(null);
  });

  it('follows nothing when the screen names no place', () => {
    useFinder.setState({ open: true });
    renderHook(() => useFinderTreeFeed(null));
    expect(followed).toHaveBeenLastCalledWith(null);
  });
});
