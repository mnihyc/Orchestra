/** The shell and its early readers use the same CSS-defined viewport shape. */
export type Shape = 'wide' | 'medium' | 'small';

export function viewportQuery(token: '--screen-md' | '--screen-lg' | '--breakpoint-sm', fallback: string): MediaQueryList | null {
  if (typeof matchMedia !== 'function') return null;
  const width = getComputedStyle(document.documentElement).getPropertyValue(token).trim() || fallback;
  return matchMedia(`(min-width: ${width})`);
}

let medium: MediaQueryList | null | undefined;
let large: MediaQueryList | null | undefined;

export function viewportShape(): Shape {
  medium ??= viewportQuery('--screen-md', '900px');
  large ??= viewportQuery('--screen-lg', '1200px');
  if (medium === null || large === null) return 'wide';
  return large.matches ? 'wide' : medium.matches ? 'medium' : 'small';
}

export function watchViewportShape(listener: () => void): () => void {
  viewportShape();
  medium?.addEventListener('change', listener);
  large?.addEventListener('change', listener);
  return () => {
    medium?.removeEventListener('change', listener);
    large?.removeEventListener('change', listener);
  };
}
