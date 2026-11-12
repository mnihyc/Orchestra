import { useCallback, useEffect, useState } from 'react';

/**
 * Whether a part of the page has been seen, and true from then on. A part below the first screen asks
 * the server for what it shows only then, so that opening a page costs the reads of what is seen first.
 *
 * It is seen when it comes into view (also when it is in view as the page opens: the observer reports that at once), and
 * when the reader's focus enters it or the part that holds it, which is how a keyboard or a heading jump reaches a part
 * that the view has not yet scrolled to; the third member is for that. Without a way to watch the view (a test document)
 * the part counts as seen.
 */
export function useSeen(): [(element: Element | null) => void, boolean, () => void] {
  const [element, setElement] = useState<Element | null>(null);
  const [seen, setSeen] = useState(() => typeof IntersectionObserver === 'undefined');
  const see = useCallback(() => setSeen(true), []);
  useEffect(() => {
    if (seen || element === null) return;
    const observer = new IntersectionObserver((entries) => { if (entries.some((entry) => entry.isIntersecting)) setSeen(true); });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, seen]);
  return [setElement, seen, see];
}
