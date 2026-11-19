/** A view put into another area: its tab dragged onto a group, an edge or a bar; picked up, carried from target to target and dropped by keyboard; and Move to in its menu.
 *
 * There are three ways to move a view and one thing they all come to: a target of the layout tree.
 * A drop spot is a target with a name. The pointer picks the spot it is over, the most particular
 * one where several lie on one another. The keyboard carries a tab from spot to spot in the
 * direction of the arrow key, and a screen reader is told the name of each. The menu lists the same
 * targets in words.
 */
import {
  closestCenter,
  type CollisionDetection,
  type KeyboardCoordinateGetter,
  pointerWithin,
} from '@dnd-kit/core';
import type { TFunction } from 'i18next';

import {
  type AreaId,
  canMove,
  findTab,
  type Layout,
  mainGroups,
  type Target,
} from '@/features/workbench/layout';
import type { MenuEntry } from '@/ui/Menu';
import { containerOf } from '@/features/workbench/containers';
import { viewOf } from '@/features/workbench/views';
import { containerChoice } from '@/features/workbench/choiceWords';
import type { ChoiceHelp } from '@/ui/Choice';

/** How far a tab is dragged before it counts as dragged: nearer than this it was a click. */
export const DRAG_STARTS_AFTER_PX = 6;

/** What a drop spot carries: where a tab lands there, and what the place is called. */
export interface DropSpot {
  target: Target;
  label: string;
}

/** A name for a drop spot that is the same for the same target, however often it is drawn. */
export function spotId(target: Target): string {
  switch (target.to) {
    case 'container':
      return `container:${target.bar}:${target.container}:${target.index ?? 'end'}`;
    case 'area':
      return `area:${target.area}`;
    case 'split':
      return `split:${target.group}:${target.side}`;
    case 'group':
      return `group:${target.group}:${target.index ?? 'end'}`;
  }
}

/**
 * The spot a dragged tab is over. Under the pointer the smallest spot wins, since a tab's own place
 * lies on its group and the group on its area. A tab carried by keyboard has no pointer, and is
 * over the spot its middle is nearest to.
 */
export const overSpot: CollisionDetection = (args) => {
  if (args.pointerCoordinates === null) return closestCenter(args);
  const area = (id: string | number) => {
    const rect = args.droppableRects.get(id);
    return rect === undefined ? Number.POSITIVE_INFINITY : rect.width * rect.height;
  };
  return [...pointerWithin(args)].sort((one, other) => area(one.id) - area(other.id));
};

const DIRECTIONS: Record<string, { x: number; y: number }> = {
  ArrowLeft: { x: -1, y: 0 },
  ArrowRight: { x: 1, y: 0 },
  ArrowUp: { x: 0, y: -1 },
  ArrowDown: { x: 0, y: 1 },
};

/**
 * Where an arrow key carries a tab: to the nearest drop spot that lies in that direction. Spots
 * straight ahead are preferred to spots off to the side, so that a key goes where it points.
 */
export const carryBetweenSpots: KeyboardCoordinateGetter = (event, { context }) => {
  const direction = DIRECTIONS[event.code];
  const from = context.collisionRect;
  if (direction === undefined || from === null) return undefined;
  event.preventDefault();

  const centre = { x: from.left + from.width / 2, y: from.top + from.height / 2 };
  let best: { x: number; y: number; cost: number } | null = null;
  for (const container of context.droppableContainers.getEnabled()) {
    if (container.id === context.over?.id) continue;
    const rect = context.droppableRects.get(container.id);
    if (rect === undefined) continue;
    const dx = rect.left + rect.width / 2 - centre.x;
    const dy = rect.top + rect.height / 2 - centre.y;
    const ahead = dx * direction.x + dy * direction.y;
    const aside = Math.abs(dx * direction.y) + Math.abs(dy * direction.x);
    if (ahead <= 1) continue;
    // A step to the side counts double: straight ahead first.
    const cost = ahead + aside * 2;
    if (best === null || cost < best.cost) {
      best = {
        x: rect.left + (rect.width - from.width) / 2,
        y: rect.top + (rect.height - from.height) / 2,
        cost,
      };
    }
  }
  return best === null ? undefined : { x: best.x, y: best.y };
};

/** What an area is called. */
export function areaName(area: AreaId, t: TFunction): string {
  return t(`workbench.areas.${area}`);
}

/** What a group is called: its area, and for one of several in the main area, which. */
export function groupName(layout: Layout, groupId: string, t: TFunction): string {
  if (layout.bottom.group.id === groupId) return areaName('bottom', t);
  for (const bar of ['left', 'right'] as const) {
    const container = layout[bar].containers.find((candidate) => candidate.id === groupId);
    if (container !== undefined) return containerTitle(container.name, t);
  }
  const groups = mainGroups(layout.main);
  const index = groups.findIndex((group) => group.id === groupId);
  return groups.length < 2 || index === -1 ? areaName('main', t) : t('workbench.areas.mainGroup', { number: index + 1 });
}

/** What a drop spot is called, for a screen reader and for the mark that shows where a tab would land. */
export function spotLabel(layout: Layout, target: Target, t: TFunction): string {
  switch (target.to) {
    case 'container':
      return t('workbench.spots.into', { group: containerTitle(target.container, t) });
    case 'area':
      return areaName(target.area, t);
    case 'split':
      return t(`workbench.spots.${target.side}`, { group: groupName(layout, target.group, t) });
    case 'group':
      return t('workbench.spots.into', { group: groupName(layout, target.group, t) });
  }
}

/** "Move to" in a tab's menu: every place its view can be put, with the ones that would change nothing greyed and why. */
export function moveEntries(
  layout: Layout,
  tabId: string,
  t: TFunction,
  apply: (target: Target) => void,
  helpFor?: (name: string) => ChoiceHelp | undefined,
): MenuEntry[] {
  const place = findTab(layout, tabId);
  if (place === null) return [];
  const there = t('workbench.move.alreadyThere');
  const alone = place.group.tabs.length === 1;

  const toArea = (area: AreaId): MenuEntry => ({
    id: `move-${area}`,
    name: t(`workbench.choices.areas.${area}.name`),
    description: t(`workbench.choices.areas.${area}.description`),
    help: helpFor?.(t(`workbench.choices.areas.${area}.name`)),
    // The only tab of the main area's one group is in the main area, and so is any tab of it.
    disabled: place.area === area && (area !== 'main' || mainGroups(layout.main).length === 1) ? there : false,
    onSelect: () => apply({ to: 'area', area }),
  });

  // A tab of a bar is split off beside the first group of the main area; a tab of the main area beside its own.
  const beside = place.area === 'main' ? place.group.id : mainGroups(layout.main)[0]?.id;
  const split = (side: 'right' | 'down'): MenuEntry => {
    const target: Target | null = beside === undefined ? null : { to: 'split', group: beside, side };
    const possible = target !== null && canMove(layout, tabId, target);
    return {
      id: `split-${side}`,
      name: t(`workbench.choices.split.${side}.name`),
      description: t(`workbench.choices.split.${side}.description`, { group: beside === undefined ? areaName('main', t) : groupName(layout, beside, t) }),
      help: helpFor?.(t(`workbench.choices.split.${side}.name`)),
      disabled: possible ? false : alone && place.area === 'main' ? t('workbench.move.onlyTab') : there,
      onSelect: () => {
        if (target !== null) apply(target);
      },
    };
  };

  return [
    ...(['left', 'right'] as const).flatMap((bar) => layout[bar].containers.map((container): MenuEntry => {
      const choice = containerChoice(container.name, t);
      return { id: `move-${bar}-${container.name}`, name: t('workbench.choices.moveContainer.name', { name: choice.name }),
        description: t('workbench.choices.moveContainer.description', { area: areaName(bar, t), description: choice.description }),
        help: helpFor?.(choice.name),
        disabled: place.area === bar && place.container?.name === container.name ? there : false,
        onSelect: () => apply({ to: 'container', bar, container: container.name }),
      };
    })),
    toArea('bottom'),
    toArea('main'),
    { type: 'separator', id: 'move-separator' },
    split('right'),
    split('down'),
  ];
}

function containerTitle(name: string, t: TFunction): string {
  const definition = containerOf(name);
  const view = viewOf(name);
  return definition === undefined ? t(view?.title ?? 'workbench.containers.label') : t(definition.title);
}
