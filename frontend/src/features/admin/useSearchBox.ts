/** A search box whose text is written to the address once the typing has paused, and not at every key.
 *
 * The box starts from what the address says. What the address says later (a step back in the history) is
 * the screen's to read; the box never writes it back, so a step back is not undone by the text that was
 * typed before it.
 */
import { useEffect, useRef, useState } from 'react';

import { useDebounced } from '@/lib/debounce';

export function useSearchBox(
  fromAddress: string | undefined,
  write: (text: string | undefined) => void,
): [string, (text: string) => void] {
  const [typed, setTyped] = useState(fromAddress ?? '');
  const settled = useDebounced(typed).trim();
  const written = useRef(fromAddress ?? '');
  useEffect(() => {
    if (settled === written.current) return;
    written.current = settled;
    write(settled === '' ? undefined : settled);
  }, [settled, write]);
  return [typed, setTyped];
}
