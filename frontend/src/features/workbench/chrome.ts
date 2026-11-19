/** Sheets start below the actual wrapped chrome, including banners, rather than covering its controls. */
import { useLayoutEffect, useState } from 'react';

export function useChromeBottom(): number | undefined {
  const [bottom, setBottom] = useState<number>();
  useLayoutEffect(() => {
    const chrome = document.querySelector<HTMLElement>('[data-workbench-chrome]') ?? document.querySelector<HTMLElement>('[data-screen-titlebar]');
    if (chrome === null) return;
    const measure = () => {
      const box = chrome.getBoundingClientRect();
      if (box.height <= 0) return;
      // Round outward by a CSS pixel so fractional text painting stays above the sheet.
      const next = Math.max(0, Math.ceil(box.bottom));
      setBottom((previous) => previous === next ? previous : next);
    };
    measure();
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    observer?.observe(chrome);
    window.addEventListener('resize', measure);
    return () => { observer?.disconnect(); window.removeEventListener('resize', measure); };
  }, []);
  return bottom;
}
