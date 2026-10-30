/** Whether the finder is open, and what it was opened with: the one switch that the key, the rail's magnifier and the commands of other screens all press. */
import { create } from 'zustand';

interface FinderState {
  open: boolean;
  participating: boolean;
  /** What the field holds when it opens: empty, or a sign that keeps it to one kind of answer. */
  seed: string;
  /** A command chooser can narrow the list without changing the words the person types. */
  commandPrefix: string | null;
  /** Counts the openings, so that opening it again with the same seed starts the field anew. */
  opened: number;
}

export const useFinder = create<FinderState>(() => ({ open: false, participating: false, seed: '', commandPrefix: null, opened: 0 }));

/** Opens the finder, with the field empty or beginning with `seed` (`>` opens it on the commands). */
export function openFinder(seed = '', commandPrefix: string | null = null): void {
  useFinder.setState((state) => ({ open: true, seed, commandPrefix, opened: state.opened + 1 }));
}

export function closeFinder(): void {
  useFinder.setState({ open: false });
}

/** Opens it when it is shut and shuts it when it is open: what its key does. */
export function toggleFinder(): void {
  if (useFinder.getState().open) closeFinder();
  else openFinder();
}
