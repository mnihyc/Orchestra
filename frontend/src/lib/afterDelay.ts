/** Something done a moment later, for a component that put a note up and takes it down again ("Saved", a row lit by a link). */
import { useCallback, useEffect, useRef } from 'react';

/**
 * Gives `after(act, ms)`: `act` runs once `ms` have passed, unless the component has gone by then (a timer nothing holds would write to
 * a component that is no longer there, and in a test run, to a window that no longer is). It returns a way to cancel that one wait.
 * What acts on something that outlives the component, such as a dialog handed over once this one has closed, needs no such hook.
 */
export function useAfterDelay(): (act: () => void, delayMs: number) => () => void {
  const waiting = useRef(new Set<ReturnType<typeof setTimeout>>());
  const gone = useRef(false);
  useEffect(() => {
    const held = waiting.current;
    gone.current = false;
    return () => {
      gone.current = true;
      for (const timer of held) clearTimeout(timer);
      held.clear();
    };
  }, []);
  return useCallback((act, delayMs) => {
    // A request that arrives after the component went (a write that was acknowledged late) has nobody to be done for.
    if (gone.current) return () => undefined;
    const held = waiting.current;
    const timer = setTimeout(() => { held.delete(timer); act(); }, delayMs);
    held.add(timer);
    return () => { clearTimeout(timer); held.delete(timer); };
  }, []);
}
