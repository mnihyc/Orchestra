/** The reads-again of a wait that has no query of its own to hang them on: `read` is called while `waiting` is true, as often as `readAgainAfter` says. */
import { useEffect, useRef } from 'react';

import { readAgainAfter } from '@/api/readAgain';

export function useReadAgain(waiting: boolean, read: () => void, whileUpMs?: number): void {
  const latest = useRef(read);
  useEffect(() => { latest.current = read; });
  useEffect(() => {
    if (!waiting) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const next = () => { timer = setTimeout(() => { latest.current(); next(); }, readAgainAfter(whileUpMs)); };
    next();
    return () => clearTimeout(timer);
  }, [waiting]);
}
