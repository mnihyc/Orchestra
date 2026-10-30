/** A scrolling view is its children's containing block, so hidden labels cannot escape into its pane. */
import type { ComponentProps } from 'react';
import { cx } from '@/ui/cx';

export const scrollAreaClasses = 'relative min-h-0 min-w-0 overflow-y-auto';

export function ScrollArea({ className, ...props }: ComponentProps<'div'>) {
  return <div {...props} data-scroll-area className={cx(scrollAreaClasses, className)} />;
}
