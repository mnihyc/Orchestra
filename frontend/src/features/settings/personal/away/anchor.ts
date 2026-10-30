/** Open an existing notification editor link at its requested card once the current projection is drawn. */
import { useLocation } from '@tanstack/react-router';
import { useEffect, useRef } from 'react';

export function useAwayAnchor(projection: number): void {
  const hash = useLocation({ select: (location) => location.hash });
  const visited = useRef<string | null>(null);
  useEffect(() => {
    if ((hash !== 'outside' && !hash.startsWith('route-')) || visited.current === hash) return;
    const frame = requestAnimationFrame(() => {
      const target = document.getElementById(hash);
      if (target === null) return;
      visited.current = hash;
      target.scrollIntoView({ block: 'start' });
      target.tabIndex = -1;
      target.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(frame);
  }, [hash, projection]);
}
