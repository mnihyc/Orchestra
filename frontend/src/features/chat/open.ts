/** Goes to an address inside the application through the router, so that the page is not loaded again.
 *
 * A link in a message, a quoted item and the name of a source all lead somewhere in the application,
 * at an address the server or the page built as a path with its search. The router is asked to go
 * there, which a plain link would do by loading the page anew.
 */
import { type RegisteredRouter, useRouter } from '@tanstack/react-router';
import { useCallback } from 'react';

/** Whether a click is an ordinary one: with a modifier or another button the browser's own link behaviour (a new tab) is wanted. */
export function isPlainClick(event: { defaultPrevented: boolean; button: number; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean }): boolean {
  return !event.defaultPrevented && event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

/**
 * A function that goes to a path of the application, such as `/topic/ab12?panel=chat`. Drawn outside a
 * router (a part shown on its own), it loads the address the way a plain link does.
 */
export function useOpenPath(): (path: string) => void {
  const router = useRouter({ warn: false }) as RegisteredRouter | undefined;
  return useCallback(
    (path: string) => {
      if (router === undefined) location.assign(path);
      else router.history.push(path);
    },
    [router],
  );
}
