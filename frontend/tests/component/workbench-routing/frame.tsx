/** The real router hosts one real workbench. Leaf bodies are mounted solely by PageView. */
import type { ReactNode } from 'react';
import { PageRouting } from '@/features/workbench/PageRouting';
import { Board, ViewHost } from '@/features/workbench/Board';
import { useVisibleLayout } from '@/features/workbench/availability';
import { WorkspaceCommands } from '@/features/workbench/workspaceActions';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { useCurrentHeading } from '@/shell/screen';

export function RoutingFrame({ children: _children }: { children: ReactNode }) {
  const layout = useVisibleLayout();
  const heading = useCurrentHeading();
  return <FrameReadiness.Provider value={{ ready: true, pending: null }}>
    <PageRouting ready />
    <WorkspaceCommands />
    <ViewHost layout={layout} />
    <Board rail={null} top={<output data-testid="focused-caption">{heading?.title}</output>} status={null} />
  </FrameReadiness.Provider>;
}
