/** Only presentation loads before the shell paints; a failed application transfer stays retryable. */
import { useCallback, useLayoutEffect, useMemo, useState } from 'react';
import { words } from 'virtual:orch-first-shell-words';
import type { Locale } from '@/generated/api-types';
import { pageHtmlReads } from '@/api/htmlReads';
import { mark } from '@/lib/marks';
import { readShellGeometry, useCover } from './keptShell';
import { ShellPicture } from './ShellPicture';

export function FirstShell({ locale, load }: { locale: Locale; load: () => Promise<unknown> }) {
  const covering = useCover((state) => state.covering);
  const geometry = useMemo(() => readShellGeometry(), []);
  const [failed, setFailed] = useState(false);
  const [handedOff, setHandedOff] = useState(false);
  const start = useCallback(() => { void load().then(() => setHandedOff(true), () => setFailed(true)); }, [load]);
  useLayoutEffect(() => {
    let next = 0;
    const first = requestAnimationFrame(() => {
      next = requestAnimationFrame(() => {
        mark('start/shell-preview-painted');
        pageHtmlReads()?.painted();
        start();
      });
    });
    return () => { cancelAnimationFrame(first); cancelAnimationFrame(next); };
  }, [start]);
  if (handedOff || !covering) return null;
  return <div data-first-shell>
    {failed ? <div role="alert" className="fixed inset-0 z-50 flex items-center justify-center gap-3 bg-bg p-4 text-sm text-text">
      <span>{words[locale].failed}</span><button type="button" className="rounded-md border border-border px-3 py-1.5" onClick={() => { setFailed(false); start(); }}>{words[locale].retry}</button>
    </div> : <ShellPicture kept={null} geometry={geometry} />}
  </div>;
}
