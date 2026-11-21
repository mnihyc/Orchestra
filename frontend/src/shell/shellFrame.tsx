/** The signed-in shell's code can start loading while the session is being restored. */
import { type ComponentProps, useState } from 'react';
import { lazyPart } from '@/features/files/lazy';
import type { Shell } from './Shell';

let prepared: typeof Shell | undefined;

export async function loadShellFrame() {
  const module = await import('./Shell');
  prepared = module.Shell;
  return module;
}

const PendingShell = lazyPart<ComponentProps<typeof Shell>>(
  () => loadShellFrame().then((module) => ({ default: module.Shell })),
);

export function ShellFrame(props: ComponentProps<typeof Shell>) {
  // A cold frame keeps its lazy wrapper after the import finishes, preserving its mounted views.
  const [Frame] = useState(() => prepared ?? PendingShell);
  return <Frame {...props} />;
}
