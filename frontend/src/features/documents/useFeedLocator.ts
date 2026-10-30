/** A directory's content may be retired while its same reader still owns the opaque feed locator. */
import { useState } from 'react';

export function useFeedLocator(scope: string, feed: string | null | undefined): string | null | undefined {
  const [held, setHeld] = useState({ scope, feed });
  const current = feed === undefined && held.scope === scope ? held.feed : feed;
  // A changed reader or directory cannot inherit the former locator. Only absent content
  // within this exact owner keeps it; an explicit native null ends the live source.
  if (held.scope !== scope || current !== held.feed) setHeld({ scope, feed: current });
  return current;
}
