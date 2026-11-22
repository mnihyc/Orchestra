import { useEffect, useState } from 'react';

import { pageHtmlReads } from '@/api/htmlReads';

/** An addressed conversation paints its first screen before optional readers start. */
export function useThreadOpeningPaint(): boolean {
  const [firstPaint, setFirstPaint] = useState(() => pageHtmlReads()?.afterThreadContent);
  useEffect(() => {
    if (firstPaint === undefined) return;
    let mounted = true;
    void firstPaint.then(() => { if (mounted) setFirstPaint(undefined); });
    return () => { mounted = false; };
  }, [firstPaint]);
  return firstPaint === undefined;
}
