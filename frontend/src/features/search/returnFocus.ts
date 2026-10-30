/** Find and result tabs return the keyboard to the same connected control and its saved caret or text selection. */
export interface ReturnFocus {
  element: HTMLElement | null;
  range: Range | null;
  field: { start: number; end: number; direction: 'forward' | 'backward' | 'none' } | null;
}

/** Captures the current control in a view, or its root when the focus is outside it. */
export function captureFocus(root?: HTMLElement): ReturnFocus {
  const active = document.activeElement;
  const element = active instanceof HTMLElement && (root === undefined || root.contains(active)) ? active : root ?? null;
  const field = element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement ? element : null;
  const start = field?.selectionStart ?? null;
  const end = field?.selectionEnd ?? null;
  const selected = document.getSelection();
  const range = selected !== null && selected.rangeCount > 0 ? selected.getRangeAt(0) : null;
  return {
    element,
    field: start === null || end === null ? null : { start, end, direction: field?.selectionDirection ?? 'none' },
    range: range !== null && element?.contains(range.commonAncestorContainer) === true ? range.cloneRange() : null,
  };
}

/** Restores an opener that still exists without scrolling away from the content it was reading. */
export function restoreFocus(saved: ReturnFocus | undefined | null): void {
  if (saved?.element?.isConnected !== true) return;
  const { element, field, range } = saved;
  element.focus({ preventScroll: true });
  if (field !== null && (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) {
    element.setSelectionRange(field.start, field.end, field.direction);
  } else if (range?.commonAncestorContainer.isConnected === true) {
    const selected = document.getSelection();
    selected?.removeAllRanges();
    selected?.addRange(range);
  }
}
