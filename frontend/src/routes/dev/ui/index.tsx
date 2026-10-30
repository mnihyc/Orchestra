/** Every shared control that is built, in both themes, both densities and both languages: for whoever works on them.
 *
 * It exists in a development build only. A build that is deployed has the address and nothing behind
 * it: the page is not part of that build, and the route says that there is nothing there.
 */
import { createFileRoute, notFound } from '@tanstack/react-router';
import { lazy, Suspense } from 'react';

import { SkeletonRows } from '@/ui/Skeleton';

const Gallery = import.meta.env.DEV ? lazy(() => import('./-Gallery')) : null;

export const Route = createFileRoute('/dev/ui/')({
  // The gallery fills the page by itself. Where it is not built, the address is framed like any
  // other address that leads nowhere.
  staticData: { needs: 'any', ...(Gallery === null ? {} : { frame: 'bare' as const }) },
  beforeLoad: () => {
    if (Gallery === null) throw notFound();
  },
  component: DevUi,
});

function DevUi() {
  if (Gallery === null) return null;
  return (
    <Suspense fallback={<SkeletonRows rows={8} />}>
      <Gallery />
    </Suspense>
  );
}
