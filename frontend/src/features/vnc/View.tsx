/** Where the VNC view stands: beside a thread, registered only for a reader who may watch the displays of its environment and only where they are on, and as the VNC tab of an environment's page.
 *
 * Both fetch the view's code when they are first drawn, so that no other screen carries it or the viewer
 * it draws with. What the view needs is asked of the thread's environment, never of the thread's own page.
 * The row of the agent's tool brings this view to the front on the display its answer names
 * (`features/browser/View`).
 */
import { MonitorPlay } from 'lucide-react';
import { useEffect } from 'react';

import { lazyPart } from '@/features/files/lazy';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { DisplaysSection } from '@/features/workbench/TargetSections';

import type { DisplaysPanelProps } from './Displays';

/** The VNC tab of an environment's page, fetched when it is first drawn. */
export const DisplaysTab = lazyPart<DisplaysPanelProps>(() => import('./Displays').then((module) => ({ default: module.DisplaysPanel })));

/** The view's body beside a thread, fetched when it is first drawn. */
const VncViewBody = lazyPart<ViewProps>(() => import('./Panel').then((module) => ({ default: module.VncView })));

/** Registers the view beside a thread whose environment lets the reader watch its displays, where they are on. */
export function useVncRegistration(): void {
  useEffect(() => {
    const viewer = registerView({ name: 'vnc', title: 'vnc.view.title', about: 'vnc.view.about', icon: MonitorPlay, screens: ['thread'], area: 'main', body: VncViewBody });
    const section = registerView({ name: 'displays', title: 'workbench.targets.displays', about: 'vnc.view.about', icon: MonitorPlay, screens: ['thread'], area: 'right', container: 'environment', order: 5, body: DisplaysSection });
    return () => { section(); viewer(); };
  }, []);
}
