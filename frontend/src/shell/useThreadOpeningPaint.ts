import { useEffect, useState } from 'react';

import { pageHtmlReads } from '@/api/htmlReads';

/** A conversation paints its first screen before optional readers start, also when it is where another page landed. */
export function useThreadOpeningPaint(): boolean {
  const [firstPaint, setFirstPaint] = useState(() => pageHtmlReads()?.afterThreadContent ?? pageHtmlReads()?.afterFirstContent);
  useEffect(() => {
    if (firstPaint === undefined) return;
    let mounted = true;
    void firstPaint.then(() => { if (mounted) setFirstPaint(undefined); });
    return () => { mounted = false; };
  }, [firstPaint]);
  return firstPaint === undefined;
}
