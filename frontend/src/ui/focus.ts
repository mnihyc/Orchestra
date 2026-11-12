/** Restoring the keyboard does not ask for the focused control's hover hint. */
export const AUTOMATIC_FOCUS = 'data-automatic-focus';

export function quietFocusScope(element: HTMLElement): () => void {
  const before = element.getAttribute(AUTOMATIC_FOCUS);
  element.setAttribute(AUTOMATIC_FOCUS, '');
  return () => {
    if (before === null) element.removeAttribute(AUTOMATIC_FOCUS);
    else element.setAttribute(AUTOMATIC_FOCUS, before);
  };
}

export function restoreFocusQuietly(element: HTMLElement): void {
  const release = quietFocusScope(element);
  try { element.focus(); } finally { release(); }
}
