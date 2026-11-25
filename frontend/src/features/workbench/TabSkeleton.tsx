/** What a tab's body shows until its read answers: the shape of that kind of page, not a list of lines for every page.
 *
 * A conversation shows messages, one after another and from either side, with its box below; a list shows its heading, its
 * filter and its rows; any other page shows its heading and its sections. The shape says where things will be, and says
 * nothing else: a screen reader is told once that the tab is loading, by the group these shapes stand in.
 */
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { cx } from '@/ui/cx';

import { contentLocation, parsePageAddress } from './contentAddress';
import type { Tab } from './layout';

export type TabShape = 'conversation' | 'list' | 'page';

/** The shape of the page a route draws: a thread is a conversation, a route that names one thing is a page, one that names none is a list. */
export function shapeOfRoute(route: string | null): TabShape {
  if (route === null) return 'page';
  if (route === '/thread/$threadId/' || route === '/chat/$chatId/' || route === '/place-chat/$nodeId/') return 'conversation';
  return route.includes('$') ? 'page' : 'list';
}

/** The shape a tab will draw, from the address it was opened for. */
export function shapeOfTab(tab: Tab): TabShape {
  const location = contentLocation(tab);
  return location?.view === 'page' ? shapeOfRoute(parsePageAddress(location.href)?.route ?? null) : 'page';
}

function Conversation() {
  return (
    <div className="flex h-full min-h-0 flex-col gap-3 p-4">
      <div className="flex min-h-0 flex-1 flex-col justify-end gap-4 overflow-hidden">
        {[0, 1, 2, 3].map((message) => (
          <div key={message} className={cx('flex items-start gap-2', message % 2 === 1 && 'flex-row-reverse')}>
            <Skeleton className="size-6 shrink-0 rounded-full" />
            <div className={cx('flex w-2/3 flex-col gap-1.5', message % 2 === 1 && 'items-end')}>
              <Skeleton className="h-3 w-full" />
              <Skeleton className="h-3 w-3/4" />
            </div>
          </div>
        ))}
      </div>
      <Skeleton className="h-16 w-full shrink-0 rounded-lg" />
    </div>
  );
}

function PageShape() {
  return (
    <div className="flex flex-col gap-4 p-4">
      <Skeleton className="h-6 w-1/3" />
      <div className="flex gap-3">{[0, 1, 2].map((tab) => <Skeleton key={tab} className="h-4 w-16" />)}</div>
      {[0, 1].map((section) => (
        <div key={section} className="flex flex-col gap-2">
          <Skeleton className="h-4 w-1/4" />
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-5/6" />
        </div>
      ))}
    </div>
  );
}

export function TabSkeleton({ shape, className }: { shape: TabShape; className?: string }) {
  if (shape === 'list') {
    return (
      <SkeletonGroup className={cx('flex flex-col gap-3 p-3', className)}>
        <Skeleton className="h-6 w-40" />
        <Skeleton className="h-8 w-64" />
        {[0, 1, 2, 3, 4, 5, 6, 7].map((row) => (
          <div key={row} className="flex h-(--row-height) items-center gap-3">
            <Skeleton className="size-4 shrink-0" />
            <Skeleton className={cx('h-3', row % 2 === 0 ? 'w-1/4' : 'w-1/3')} />
            <Skeleton className="ms-auto h-3 w-1/6" />
          </div>
        ))}
      </SkeletonGroup>
    );
  }
  return <SkeletonGroup className={cx('h-full', className)}>{shape === 'conversation' ? <Conversation /> : <PageShape />}</SkeletonGroup>;
}
