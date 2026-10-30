/** Measures complete container labels to choose names or icons; every control remains visible. */
import { useLayoutEffect, useRef, useState } from 'react';

export function labelsNeedIcons(widths: readonly number[], room: number): boolean {
  return widths.reduce((sum, width) => sum + width, 0) > room;
}

/** Put inert, intrinsically sized full labels in `measurement`, with data-compact-label. */
export function useCompactStrip(labels: readonly string[]) {
  const frame = useRef<HTMLDivElement>(null);
  const measurement = useRef<HTMLDivElement>(null);
  const signature = labels.join('\u0000');
  const [compact, setCompact] = useState(false);
  useLayoutEffect(() => {
    const element = frame.current;
    const ruler = measurement.current;
    if (element === null || ruler === null) return;
    const measure = () => {
      const widths = Array.from(ruler.querySelectorAll<HTMLElement>('[data-compact-label]'), (item) => Math.ceil(item.getBoundingClientRect().width));
      const style = getComputedStyle(element);
      const room = element.clientWidth - (Number.parseFloat(style.paddingLeft) || 0) - (Number.parseFloat(style.paddingRight) || 0);
      setCompact(labelsNeedIcons(widths, Math.max(0, room)));
    };
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    for (const item of ruler.children) observer.observe(item);
    const first = requestAnimationFrame(measure);
    return () => { cancelAnimationFrame(first); observer.disconnect(); };
  }, [signature]);
  return { frame, measurement, compact };
}
