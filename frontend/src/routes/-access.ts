/** What a route needs of whoever opens it, what stands round its screen, and the one place both are enforced.
 *
 * Every route says in its static data whom it is for. Before any of them is entered the root route
 * asks two things at once, what the system says of itself and whether this browser holds a session,
 * and sends the visitor on where the route is not for them: to the setup while the system has no
 * user, to the sign-in when a session is needed, into the application when one is not wanted.
 */
import { redirect } from '@tanstack/react-router';
import { QueryObserver } from '@tanstack/react-query';

import { metaQuery, signOut } from '@/api/auth';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { setSystemName } from '@/lib/title';

/** Whom a route is for. */
export type RouteNeeds =
  /** Somebody signed in. What they may see there is decided by the screen's own requests. */
  | 'signed_in'
  /** Nobody signed in: whoever is, is sent into the application. */
  | 'signed_out'
  /** A system that has no user yet. */
  | 'setup'
  /** Anybody. */
  | 'any';

/** What stands round a screen: the workbench, the plain page of the screens before sign-in, or nothing at all. */
export type RouteFrame = 'shell' | 'plain' | 'bare';

/** The kinds of screen there are: the workbench keeps one arrangement for each. */
export type ScreenKind = 'org' | 'project' | 'topic' | 'thread' | 'document' | 'page';

declare module '@tanstack/react-router' {
  interface StaticDataRouteOption {
    /** Whom the route is for; somebody signed in when absent. */
    needs?: RouteNeeds;
    /** What stands round its screen; the workbench when absent. */
    frame?: RouteFrame;
    /** The kind of screen, by which the workbench is arranged; a plain page when absent. */
    screen?: ScreenKind;
  }
}

/**
 * A path inside the application to go on to after signing in, or `null` when what was given is
 * anything else: an address elsewhere would let a link send somebody away under this site's name.
 */
export function safeRedirect(value: unknown): string | null {
  if (typeof value !== 'string' || !value.startsWith('/') || value.startsWith('//') || value.includes('\\')) return null;
  try {
    const url = new URL(value, location.origin);
    if (url.origin !== location.origin) return null;
    return `${url.pathname}${url.search}${url.hash}`;
  } catch {
    return null;
  }
}

/** Set while somebody signs out on purpose: they are then taken to the sign-in as it is, with no way back named. */
let leaving = false;

/**
 * Signs out because the person asked to. Unlike a session that ended by itself, this leads to the
 * plain sign-in, and not back to the screen that was in view.
 */
export async function leave(): Promise<void> {
  leaving = true;
  try {
    await signOut();
  } catch (error) {
    leaving = false;
    throw error;
  }
}

interface Visit {
  /** What the route to be entered needs; `undefined` for a path no route answers. */
  needs: RouteNeeds | undefined;
  /** Where the visitor is going, as the path with its query and fragment. */
  href: string;
  pathname: string;
  /** The `redirect` of the address, where there is one. */
  onward: unknown;
}

async function admissionFacts() {
  // Admission still owns Meta when restoration unmounts the last AuthFrame observer.
  // Disabled retains without fetching; Infinity does not block a caller-specific refetch.
  const reader = new QueryObserver(queryClient, { ...metaQuery, enabled: false, staleTime: Infinity });
  const release = reader.subscribe(() => undefined);
  try {
    return await Promise.all([queryClient.query({ ...metaQuery, staleTime: 'static' }), client.restore()]);
  } finally { release(); }
}

/**
 * Sends the visitor on where the route is not for them. Throws the redirect; returns when they may
 * enter. A failure to ask is thrown as it is, and the root route shows it with a way to try again.
 */
export async function admit(visit: Visit): Promise<void> {
  const [meta, signedIn] = await admissionFacts();
  setSystemName(meta.name);

  // A system with no user has one screen, and everything leads to it.
  if (meta.needs_setup) {
    if (visit.needs === 'setup') return;
    throw redirect({ to: '/setup', replace: true });
  }

  switch (visit.needs ?? (signedIn ? 'signed_in' : 'any')) {
    case 'setup':
      throw redirect({ to: signedIn ? '/' : '/login', replace: true });
    case 'signed_out':
      if (signedIn) throw redirect({ href: safeRedirect(visit.onward) ?? '/', replace: true });
      return;
    case 'signed_in': {
      if (signedIn) return;
      const onPurpose = leaving;
      leaving = false;
      throw redirect({
        to: '/login',
        // The root needs no saying: it is where a sign-in leads anyway.
        search: onPurpose || visit.pathname === '/' ? {} : { redirect: visit.href },
        replace: true,
      });
    }
    case 'any':
  }
}
