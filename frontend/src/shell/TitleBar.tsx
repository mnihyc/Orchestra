/** The one title-bar position for the linked screen path at every window width. */
import type { ReactNode } from 'react';

import { useFrameReady } from '@/shell/FrameReadiness';
import { Breadcrumbs } from '@/features/navigation/Breadcrumbs';
import { AdminModeControl } from '@/features/access/AdminMode';
import { AddViewButton } from '@/features/workbench/OpenViews';

export function ScreenTitleBar({ before, children }: { before?: ReactNode; children?: ReactNode }) {
  const ready = useFrameReady();
  return (
    <header data-screen-titlebar data-area-stop aria-busy={!ready} className="flex min-h-(--control-md) shrink-0 items-center gap-1 border-b border-border bg-bg px-1 sm:px-3 [&:not(:has([data-screen-path]))_button>svg]:-translate-y-px">
      {before}
      <Breadcrumbs className="min-w-0 flex-1 px-1" />
      {children}
      <AddViewButton />
      <AdminModeControl />
    </header>
  );
}
