/** One shell listener and the finder binding exist before bootstrap or a routed screen finishes. */
import { useQuery } from '@tanstack/react-query';
import { useLayoutEffect } from 'react';

import { meStateQuery } from '@/api/personal';
import { toggleFinder, useFinder } from '@/features/finder/store';
import { useHelpState } from '@/features/help/state';
import { keyChoicesOf } from '@/shell/keyActions';
import { bindAction, keyBook, setKeyChoices } from '@/shell/keys';

export function useShellKeyboard(active: boolean): void {
  // Bootstrap hydrates this existing record. A shortcut never adds a startup state request.
  const keys = useQuery({ ...meStateQuery, enabled: false, select: (state) => state.keys }).data;
  useLayoutEffect(() => { if (active) setKeyChoices(keyChoicesOf(keys)); }, [active, keys]);
  useLayoutEffect(() => {
    if (!active) return;
    const removeFinder = bindAction('finder', () => {
      if (useHelpState.getState().overlay && useFinder.getState().open) {
        useHelpState.setState({ overlay: false });
        return true;
      }
      toggleFinder();
    });
    const onKey = (event: KeyboardEvent) => { keyBook.handle(event); };
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('keydown', onKey); removeFinder(); };
  }, [active]);
}
