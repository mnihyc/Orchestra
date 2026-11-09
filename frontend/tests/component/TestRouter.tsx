/** A real isolated routing context for components whose names and group headers contain native links. */
import * as Routing from '@tanstack/react-router';
import { useState, type ReactNode } from 'react';

export function TestRouter({ children }: { children: ReactNode }) {
  const [router] = useState(() => {
    // Files that deliberately replace the entire router module own their own link/navigation double.
    if (!('createRootRoute' in Routing) || !('createRouter' in Routing)
      || !('createMemoryHistory' in Routing) || !('RouterContextProvider' in Routing)) return null;
    return Routing.createRouter({ routeTree: Routing.createRootRoute(), history: Routing.createMemoryHistory({ initialEntries: ['/'] }) });
  });
  return router === null ? children : <Routing.RouterContextProvider router={router}>{children}</Routing.RouterContextProvider>;
}
