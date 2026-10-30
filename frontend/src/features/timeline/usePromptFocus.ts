/** One explicit version-arrow intent follows its prompt through a bounded async history load. */
import { useEffect, useRef, type RefObject } from 'react';
import type { PromptFocus } from './context';

export function usePromptFocus(root: RefObject<HTMLElement | null>, request: PromptFocus | null | undefined,
  rows: readonly string[]): void {
  const settled = useRef<PromptFocus | null>(null);
  useEffect(() => {
    const element = root.current;
    if (request === null || request === undefined || settled.current === request || element === null) return;
    let observer: MutationObserver | null = null;
    const stop = () => {
      observer?.disconnect();
      document.removeEventListener('pointerdown', cancel, true);
      document.removeEventListener('keydown', cancel, true);
    };
    const cancel = () => { settled.current = request; stop(); };
    const focus = () => {
      if (document.activeElement instanceof Element && document.activeElement.closest('[role="dialog"], [role="alertdialog"]') !== null) {
        cancel(); return;
      }
      const group = [...element.querySelectorAll<HTMLElement>('[data-prompt-group]')].find((one) => one.dataset.promptGroup === request.groupId);
      const button = group?.querySelector<HTMLButtonElement>('button:not(:disabled)');
      if (button === null || button === undefined) return;
      settled.current = request; stop(); button.focus({ preventScroll: true });
    };
    document.addEventListener('pointerdown', cancel, true);
    document.addEventListener('keydown', cancel, true);
    observer = new MutationObserver(focus);
    observer.observe(element, { childList: true, subtree: true });
    focus();
    return stop;
  }, [root, request, rows]);
}
