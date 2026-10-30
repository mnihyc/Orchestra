/** Shared Workbench component (workbench): the areas of the shell and the borders between them.
 *
 * The frame of the workbench: a rail that never moves, a bar at the left, the main area with a bar
 * under it and a bar at its right, and one line at the foot. The borders between them are dragged,
 * or moved with the arrow keys. A bar keeps its size in pixels when the window is resized, since it
 * holds a list or a panel that wants a width; the main area takes what is left.
 *
 * A bar that is closed stays in the frame at no size, so that opening and closing it moves nothing
 * else and loses nothing; dragging its border out opens it again.
 */
import { type ReactNode, type RefObject, useEffect, useState } from 'react';
import { Group, type LayoutChangedMeta, Panel, type PanelImperativeHandle, usePanelRef } from 'react-resizable-panels';

import { cx } from '@/ui/cx';
import { Sash } from '@/ui/Sash';

export interface BarSpec {
  open: boolean;
  /** Its size in pixels: a width, or for the bar at the bottom a height. */
  size: number;
  /** The least and the most it can be, as lengths ("13.75rem", "50%"). */
  min: string;
  max: string;
  /** What the border that sizes it is called, for a screen reader. */
  sashLabel: string;
  /** False where the bar holds nothing: its border is then drawn and cannot be moved. */
  usable: boolean;
  content: ReactNode;
  onOpenChange: (open: boolean) => void;
  onResize: (size: number) => void;
}

export interface WorkbenchProps {
  /** A display override; the caller retains the normal bar sizes and visibility. */
  focusMode?: boolean;
  rail: ReactNode;
  /** What spans the main area and the bar at its right, above both: notices, and the title of the main area. */
  top?: ReactNode;
  left: BarSpec;
  /** `null` where the screen has no room for a bar beside the main area: it is then shown some other way. */
  right: BarSpec | null;
  bottom: BarSpec;
  main: ReactNode;
  status: ReactNode;
  className?: string;
}

/** The least the main area keeps, whatever the bars ask for: room for a column of text. */
const MAIN_MIN = '20rem';
/** The least height of the main area above the bottom bar: a tab strip and a few lines. */
const MAIN_MIN_HEIGHT = '8rem';

/** Keeps a bar's panel as open, and as large, as the layout says. */
function useBar(spec: BarSpec | null): RefObject<PanelImperativeHandle | null> {
  const panel = usePanelRef();
  const open = spec?.open ?? false;
  const size = spec?.size ?? 0;
  useEffect(() => {
    const handle = panel.current;
    if (handle === null) return;
    if (open && handle.isCollapsed()) handle.resize(size);
    else if (!open && !handle.isCollapsed()) handle.collapse();
    // The size is the one to open at; a change of size alone is the reader's own dragging.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, panel]);
  return panel;
}

/**
 * Tells the layout what a bar's panel came to after its reader let its border go. Only what a reader
 * did is told: a panel that was laid out anew for another reason reports sizes that are nobody's choice.
 */
function settle(spec: BarSpec, panel: PanelImperativeHandle | null, meta: LayoutChangedMeta): void {
  if (panel === null || !meta.isUserInteraction) return;
  const collapsed = panel.isCollapsed();
  if (collapsed === spec.open) spec.onOpenChange(!collapsed);
  if (!collapsed) spec.onResize(panel.getSize().inPixels);
}

/** The sizes the bars open at when their panels are put in: what the layout says of each at that moment. */
function openingSizes(left: BarSpec, right: BarSpec | null, bottom: BarSpec): { left: number; right: number; bottom: number } {
  return { left: left.open ? left.size : 0, right: right?.open === true ? right.size : 0, bottom: bottom.open ? bottom.size : 0 };
}

export function Workbench({ rail, top, left, right, bottom, main, status, className, focusMode = false }: WorkbenchProps) {
  const leftPanel = useBar(left);
  const rightPanel = useBar(right);
  const bottomPanel = useBar(bottom);
  // The first sizes are the layout's; after that the panels keep their own and report them. Focus mode takes the
  // panels out; leaving it puts them in again, at what the layout says then (a bar opened or resized since the first
  // drawing would otherwise come back closed or at its old size).
  const [first, setFirst] = useState(() => openingSizes(left, right, bottom));
  const [focused, setFocused] = useState(focusMode);
  if (focused !== focusMode) {
    setFocused(focusMode);
    if (!focusMode) setFirst(openingSizes(left, right, bottom));
  }

  if (focusMode) return <div className={cx('flex h-dvh min-h-0 flex-col overflow-hidden bg-bg text-text', className)}>{main}</div>;

  return (
    <div className={cx('flex h-dvh min-h-0 flex-col overflow-hidden bg-bg text-text', className)}>
      <div className="flex min-h-0 flex-1">
        {rail}
        <Group orientation="horizontal" className="min-w-0 flex-1" onLayoutChanged={(_, meta) => settle(left, leftPanel.current, meta)}>
          <Panel
            id="left"
            style={{ overflow: 'hidden' }}
            panelRef={leftPanel}
            collapsible
            collapsedSize={0}
            defaultSize={first.left}
            minSize={left.min}
            maxSize={left.max}
            groupResizeBehavior="preserve-pixel-size"
            className="min-w-0"
          >
            {left.open && left.content}
          </Panel>
          <Sash id="left-sash" line="vertical" label={left.sashLabel} disabled={!left.usable} />
          <Panel id="centre" minSize={MAIN_MIN} style={{ overflow: 'hidden' }} className="flex min-w-0 flex-col">
            {top}
            <Group
              orientation="horizontal"
              className="min-h-0 flex-1"
              onLayoutChanged={(_, meta) => {
                if (right !== null) settle(right, rightPanel.current, meta);
              }}
            >
              <Panel id="middle" minSize={MAIN_MIN} style={{ overflow: 'hidden' }} className="min-w-0">
                <Group orientation="vertical" onLayoutChanged={(_, meta) => settle(bottom, bottomPanel.current, meta)}>
                  <Panel id="main" minSize={MAIN_MIN_HEIGHT} style={{ overflow: 'hidden' }} className="min-h-0">
                    {main}
                  </Panel>
                  <Sash id="bottom-sash" line="horizontal" label={bottom.sashLabel} disabled={!bottom.usable} />
                  <Panel
                    id="bottom"
                    style={{ overflow: 'hidden' }}
                    panelRef={bottomPanel}
                    collapsible
                    collapsedSize={0}
                    defaultSize={first.bottom}
                    minSize={bottom.min}
                    maxSize={bottom.max}
                    groupResizeBehavior="preserve-pixel-size"
                    className="min-h-0"
                  >
                    {bottom.open && bottom.content}
                  </Panel>
                </Group>
              </Panel>
              {right !== null && (
                <>
                  <Sash id="right-sash" line="vertical" label={right.sashLabel} disabled={!right.usable} />
                  <Panel
                    id="right"
                    style={{ overflow: 'hidden' }}
                    panelRef={rightPanel}
                    collapsible
                    collapsedSize={0}
                    defaultSize={first.right}
                    minSize={right.min}
                    maxSize={right.max}
                    groupResizeBehavior="preserve-pixel-size"
                    className="min-w-0"
                  >
                    {right.open && right.content}
                  </Panel>
                </>
              )}
            </Group>
          </Panel>
        </Group>
      </div>
      {status}
    </div>
  );
}

export interface SplitViewProps {
  /** The name of the split: its borders are named after it. */
  id: string;
  /** `row`: side by side. `column`: one above another. */
  direction: 'row' | 'column';
  /** The parts, each with its share in percent. */
  parts: readonly { id: string; share: number; content: ReactNode }[];
  /** What a border between two parts is called, for a screen reader. */
  sashLabel: string;
  /** Called with the shares of the parts, in order, after a border was let go. */
  onShares: (shares: number[]) => void;
}

/** Parts side by side or one above another, with a border between each two that is dragged. */
export function SplitView({ id, direction, parts, sashLabel, onShares }: SplitViewProps) {
  return (
    <Group
      id={id}
      orientation={direction === 'row' ? 'horizontal' : 'vertical'}
      onLayoutChanged={(layout, meta) => {
        // Only a border its reader moved is worth keeping.
        if (meta.isUserInteraction) onShares(parts.map((part) => layout[part.id] ?? part.share));
      }}
    >
      {parts.flatMap((part, index) => [
        index > 0 && (
          <Sash
            key={`${part.id}-sash`}
            id={`${part.id}-sash`}
            line={direction === 'row' ? 'vertical' : 'horizontal'}
            label={sashLabel}
          />
        ),
        <Panel key={part.id} id={part.id} defaultSize={`${part.share}%`} minSize="10%" style={{ overflow: 'hidden' }} className="min-h-0 min-w-0">
          {part.content}
        </Panel>,
      ])}
    </Group>
  );
}
