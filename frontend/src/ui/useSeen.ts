import { useEffect, useState } from 'react';

/**
 * Whether a part of the page has come into view, and true from then on. A part below the first screen asks
 * the server for what it shows only then, so that opening a page costs the reads of what is seen first.
 * Without a way to watch the view (a test document) the part counts as seen.
 */
export function useSeen(): [(element: Element | null) => void, boolean] {
  const [element, setElement] = useState<Element | null>(null);
  const [seen, setSeen] = useState(() => typeof IntersectionObserver === 'undefined');
  useEffect(() => {
    if (seen || element === null) return;
    const observer = new IntersectionObserver((entries) => { if (entries.some((entry) => entry.isIntersecting)) setSeen(true); });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, seen]);
  return [setElement, seen];
}
