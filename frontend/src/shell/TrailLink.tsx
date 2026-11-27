/** Real links retain browser gestures while ordinary application paths use the current router. */
import { type AnyRouter, type RegisteredRouter, useRouter } from '@tanstack/react-router';
import type { ComponentProps, MouseEvent, ReactNode } from 'react';

import { isPlainClick } from '@/lib/linkClick';
import type { TrailPart } from '@/shell/screen';

/** A path's existing caller may open its panel; downloads, server endpoints and other origins stay native. */
export function PathLink({ href, onClick, children, ...anchor }: ComponentProps<'a'>) {
  const router = useRouter({ warn: false }) as RegisteredRouter | undefined;
  const go = (event: MouseEvent<HTMLAnchorElement>) => {
    if (anchor['aria-disabled'] === true || anchor['aria-disabled'] === 'true') { event.preventDefault(); return; }
    onClick?.(event);
    if (router === undefined || !isPlainClick(event) || href === undefined || href.startsWith('#')
      || (anchor.target !== undefined && anchor.target !== '' && anchor.target !== '_self')
      || (anchor.download !== undefined && anchor.download !== false)) return;
    let destination: URL;
    try { destination = new URL(href, window.location.href); } catch { return; }
    if (destination.origin !== window.location.origin || !['http:', 'https:'].includes(destination.protocol)
      || destination.pathname.startsWith('/api/')) return;
    event.preventDefault();
    router.history.push(`${destination.pathname}${destination.search}${destination.hash}`);
  };
  return <a {...anchor} href={href} onClick={go}>{children}</a>;
}

/** The address a way leads to, with its previous list search state. */
export function trailHref(router: AnyRouter, part: TrailPart): string {
  return router.buildLocation({ to: part.to, ...(part.search === undefined ? {} : { search: part.search }) } as never).href;
}

export function TrailLink({ part, className, children }: { part: TrailPart; className?: string | undefined; children?: ReactNode }) {
  const router = useRouter();
  return <PathLink href={trailHref(router, part)} className={className}>{children ?? part.label}</PathLink>;
}
