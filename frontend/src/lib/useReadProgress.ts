/** Fast reads need no moving indicator; busy controls still block a duplicate act immediately. */
import { useEffect, useState } from 'react';
import { useInterfaceReading } from '@/lib/interfaceReading';

export function useReadProgress(busy: boolean): boolean {
  const { progress_delay_ms: delay } = useInterfaceReading();
  const [progress, setProgress] = useState({ busy, delay, slow: false });
  if (progress.busy !== busy || progress.delay !== delay) setProgress({ busy, delay, slow: false });
  useEffect(() => {
    if (!busy) return;
    // Browser timers use signed 32-bit milliseconds. Longer chosen delays are waited in chunks.
    const until = Date.now() + delay;
    let timer: ReturnType<typeof setTimeout>;
    const wait = () => {
      const left = until - Date.now();
      if (left <= 0) setProgress({ busy, delay, slow: true });
      else timer = setTimeout(wait, Math.min(left, 2_147_483_647));
    };
    timer = setTimeout(wait, Math.min(delay, 2_147_483_647));
    return () => clearTimeout(timer);
  }, [busy, delay]);
  return busy && progress.busy === busy && progress.delay === delay && progress.slow;
}
