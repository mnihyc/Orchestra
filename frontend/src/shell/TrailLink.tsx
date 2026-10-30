/** A way to a place above the screen in view, drawn as a real link: a plain press goes there inside the application, and a press with a modifier or the middle button opens it in a tab of the browser.
 *
 * The title bar's path and the arrow beside a heading both lead to the same places, so both are drawn
 * from the same parts. The address is built by the router from the route and what its address says,
 * so that the filter a list was left with comes back with it.
 */
import { type AnyRouter, useRouter } from '@tanstack/react-router';
import type { MouseEvent, ReactNode } from 'react';

import type { TrailPart } from '@/shell/screen';

/** The address a way leads to. */
export function trailHref(router: AnyRouter, part: TrailPart): string {
  // The route is named by a string the parts carry, so it is not one the router's types know.
  return router.buildLocation({ to: part.to, ...(part.search === undefined ? {} : { search: part.search }) } as never).href;
}

export function TrailLink({ part, className, children }: { part: TrailPart; className?: string | undefined; children?: ReactNode }) {
  const router = useRouter();
  const href = trailHref(router, part);
  const go = (event: MouseEvent<HTMLAnchorElement>) => {
    // A press that asks for a tab of the browser, or for anything else than going here, is the browser's.
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    router.history.push(href);
  };
  return (
    <a href={href} className={className} onClick={go}>
      {children ?? part.label}
    </a>
  );
}
