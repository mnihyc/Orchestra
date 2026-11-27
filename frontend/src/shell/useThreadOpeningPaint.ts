import { useEffect, useState } from 'react';

import { pageHtmlReads } from '@/api/htmlReads';

/** A conversation paints its first screen before optional readers start, also when it is where another page landed. */
export function useThreadOpeningPaint(): boolean {
  const reads = pageHtmlReads();
  return useOpeningPaint(reads?.afterThreadContent ?? reads?.afterFirstContent);
}

/** The navigation pane can fill after the content of a returning Home or inventory. */
export function useNavigationReadsReady(): boolean {
  return useOpeningPaint(pageHtmlReads()?.afterNavigationContent);
}

function useOpeningPaint(wait: Promise<void> | undefined): boolean {
  const [firstPaint, setFirstPaint] = useState(wait);
  useEffect(() => {
    if (firstPaint === undefined) return;
    let mounted = true;
    void firstPaint.then(() => { if (mounted) setFirstPaint(undefined); });
    return () => { mounted = false; };
  }, [firstPaint]);
  return firstPaint === undefined;
}
