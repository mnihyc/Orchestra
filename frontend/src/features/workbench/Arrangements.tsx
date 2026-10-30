/** The arrangements each kind of screen ships with (Conversation, Code and Review for a thread), choosing one, Reset arrangement and Save arrangement as…
 *
 * An arrangement says where the views of a screen are and which bars are open. Every kind of screen
 * ships with one, and a thread with three: the conversation in the middle to read along, the code
 * in the middle to work in it, the differences in the middle to review. They are the same views
 * each time, put where the work at hand wants them.
 *
 * So far: the shipped arrangements, choosing among them, and Reset arrangement. Saving one under a
 * name of one's own is not built yet.
 */
import { workspaceEntries } from '@/features/workbench/workspaceActions';
import { Check, LayoutPanelLeft, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  type Bar,
  type BarId,
  containersFrom,
  type Layout,
  LAYOUT_VERSION,
  makeGroup,
  makeTab,
  type SideBarId,
} from '@/features/workbench/layout';
import { viewContainer, viewOf, viewOrder } from '@/features/workbench/views';
import { placementFor } from '@/features/workbench/placement';
import type { ChoiceHelp } from '@/ui/Choice';
import type { TranslationKey } from '@/i18n';
import { useRef, useState } from 'react';
import { Popover } from '@/ui/Popover';
import { StatusCustomizer } from '@/features/status-items/Customize';
import type { ScreenKind } from '@/routes/-access';
import { IconButton } from '@/ui/IconButton';
import { Menu, DropdownEntries, type MenuEntry } from '@/ui/Menu';
import { ReadingGuideMenuItem } from '@/features/help/ReadingGuide';

export interface Arrangement {
  /** The name it is kept under. */
  name: string;
  title: TranslationKey;
  /** One sentence that says what choosing it does. */
  about: TranslationKey;
  /** The views of each area, by name; the first of each is in front. */
  left: readonly string[];
  main: readonly string[];
  right: readonly string[];
  bottom: readonly string[];
  /** Which bars begin open, where they hold something and the screen has room for them. */
  open: Readonly<Record<BarId, boolean>>;
  /** Which container each side bar shows and which sections begin expanded. */
  active?: Partial<Record<SideBarId, string>>;
  expanded?: readonly string[];
}

/** The views every thread has beside its conversation, in the order their tabs stand in. */
const THREAD_VIEWS = [
  'outline',
  'history',
  'chat',
  'documents',
  'people',
  'changes',
  'files',
  'git',
  'environment',
  'processes',
  'resources',
  'ports',
  'browsers',
  'displays',
  'parameters',
  'usage',
  'knowledge',
  'given',
  'diagnostics',
] as const;

const PLAIN: Arrangement = {
  name: 'default',
  title: 'workbench.arrangements.default.name',
  about: 'workbench.arrangements.default.description',
  left: ['navigation', 'resource-navigation'],
  main: ['screen'],
  right: [],
  bottom: [],
  open: { left: true, right: false, bottom: false },
};

const beside = (...right: string[]): Arrangement => ({ ...PLAIN, right, open: { left: true, right: true, bottom: false } });

/** The arrangements of each kind of screen. The first is the one a screen opens in. */
export const ARRANGEMENTS: Readonly<Record<ScreenKind, readonly Arrangement[]>> = {
  page: [PLAIN],
  org: [beside('chat', 'documents')],
  project: [beside('chat', 'documents')],
  topic: [beside('chat', 'files', 'environment')],
  document: [beside('comments', 'outline', 'links')],
  thread: [
    {
      name: 'conversation',
      title: 'workbench.arrangements.conversation.name',
      about: 'workbench.arrangements.conversation.description',
      left: PLAIN.left,
      main: ['screen'],
      right: THREAD_VIEWS,
      bottom: ['terminal'],
      active: { left: 'navigation', right: 'conversation' },
      expanded: ['navigation', 'resource-navigation', 'outline', 'chat', 'changes', 'environment', 'parameters'],
      open: { left: true, right: true, bottom: false },
    },
    {
      name: 'code',
      title: 'workbench.arrangements.code.name',
      about: 'workbench.arrangements.code.description',
      left: PLAIN.left,
      main: ['files'],
      right: ['screen', ...THREAD_VIEWS.filter((view) => view !== 'files')],
      bottom: ['terminal'],
      active: { left: 'navigation', right: 'conversation' },
      expanded: ['navigation', 'resource-navigation', 'screen', 'chat', 'changes', 'environment', 'parameters'],
      open: { left: true, right: true, bottom: true },
    },
    {
      name: 'review',
      title: 'workbench.arrangements.review.name',
      about: 'workbench.arrangements.review.description',
      left: PLAIN.left,
      main: ['changes'],
      right: ['screen', ...THREAD_VIEWS.filter((view) => view !== 'changes')],
      bottom: ['terminal'],
      active: { left: 'navigation', right: 'conversation' },
      expanded: ['navigation', 'resource-navigation', 'screen', 'chat', 'files', 'environment', 'parameters'],
      open: { left: true, right: true, bottom: false },
    },
  ],
};

/** The sizes the bars begin with, in pixels: what the tokens of the frame come to at the usual text size. */
export interface FrameSizes {
  left: number;
  right: number;
  bottom: number;
}

const FALLBACK_SIZES: FrameSizes = { left: 280, right: 400, bottom: 240 };

/** Reads the sizes the bars begin with from the tokens of the page, so that they are said in one place. */
export function frameSizes(): FrameSizes {
  if (typeof document === 'undefined') return FALLBACK_SIZES;
  const style = getComputedStyle(document.documentElement);
  const rem = Number.parseFloat(style.fontSize) || 16;
  const read = (token: string, fallback: number): number => {
    const value = style.getPropertyValue(token).trim();
    const number = Number.parseFloat(value);
    if (!Number.isFinite(number)) return fallback;
    return value.endsWith('rem') ? number * rem : number;
  };
  return {
    left: read('--left-bar-width', FALLBACK_SIZES.left),
    right: read('--right-bar-width', FALLBACK_SIZES.right),
    bottom: read('--bottom-bar-height', FALLBACK_SIZES.bottom),
  };
}

/** The least and the most each bar can be, as lengths the borders understand. */
export interface FrameLimits {
  left: { min: string; max: string };
  right: { min: string; max: string };
  bottom: { min: string; max: string };
}

/**
 * The bounds of the bars. Their least sizes, and the most the left bar takes, are tokens of the
 * page. The right bar and the bottom stop at a share of what they stand beside, so that the main
 * area always keeps the larger part.
 */
export function frameLimits(): FrameLimits {
  const style = typeof document === 'undefined' ? null : getComputedStyle(document.documentElement);
  const read = (token: string, fallback: string): string => {
    const value = style?.getPropertyValue(token).trim() ?? '';
    return value === '' ? fallback : value;
  };
  return {
    left: { min: read('--left-bar-min', '220px'), max: read('--left-bar-max', '480px') },
    right: { min: read('--right-bar-min', '320px'), max: RIGHT_BAR_MOST },
    bottom: { min: read('--bottom-bar-min', '120px'), max: BOTTOM_BAR_MOST },
  };
}

/** The right bar takes at most half of what it shares with the main area. */
const RIGHT_BAR_MOST = '50%';
/** The bottom takes at most seven tenths of the height: a shell may be most of the screen, never all of it. */
const BOTTOM_BAR_MOST = '70%';

export function arrangementOf(screen: ScreenKind, name?: string): Arrangement {
  const all = ARRANGEMENTS[screen];
  return all.find((arrangement) => arrangement.name === name) ?? all[0] ?? PLAIN;
}

/** The layout an arrangement comes to, with the views that are registered and that this kind of screen offers. */
export function shippedLayout(screen: ScreenKind, name?: string, sizes: FrameSizes = frameSizes()): Layout {
  const arrangement = arrangementOf(screen, name);
  const tabs = (views: readonly string[]) =>
    views
      .filter((view) => {
        const definition = viewOf(view);
        // An arrangement stores choices before its page has loaded. Runtime permissions and
        // availability project over this layout; they must not erase a starting Code/Review view.
        return definition === undefined ? ['ports', 'browsers', 'displays'].includes(view) : definition.implemented !== false && (definition.screens === 'all' || definition.screens.includes(screen));
      })
      .map((view) => makeTab(view));
  const bar = (id: 'bottom'): Bar => {
    const group = makeGroup(tabs(arrangement[id]));
    return { open: arrangement.open[id] && group.tabs.length > 0, size: sizes[id], group };
  };
  const side = (id: SideBarId) => {
    const home = (name: string) => { const definition = viewOf(name); return definition === undefined ? placementFor(name, screen).container : viewContainer(definition, screen); };
    const order = (name: string) => { const definition = viewOf(name); return definition === undefined ? placementFor(name, screen).order : viewOrder(definition, screen); };
    const containers = containersFrom(tabs(arrangement[id]), undefined, screen, home);
    for (const container of containers) container.sections.sort((one, other) => order(one.tab.view) - order(other.tab.view));
    if (arrangement.expanded !== undefined) {
      for (const container of containers) for (const section of container.sections) section.open = arrangement.expanded.includes(section.tab.view);
    }
    return { open: arrangement.open[id] && containers.length > 0, size: sizes[id], containers, active: arrangement.active?.[id] ?? containers[0]?.name ?? null };
  };
  return {
    version: LAYOUT_VERSION,
    left: side('left'),
    right: side('right'),
    bottom: bar('bottom'),
    main: makeGroup(tabs(arrangement.main)),
    maximized: null,
  };
}

/** The control in the title of the main area: the arrangements of this kind of screen, and the way back to the shipped one. */
export interface ArrangementMenuProps {
  screen: ScreenKind;
  /** The arrangement in use, by name. */
  current: string;
  onChoose: (name: string) => void;
  onReset: () => void;
  helpFor?: (name: string) => ChoiceHelp | undefined;
}

export function ArrangementMenu({ screen, current, onChoose, onReset }: ArrangementMenuProps) {
  const { t } = useTranslation();
  const all = ARRANGEMENTS[screen];
  const [statusEditing, setStatusEditing] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);

  const entries: MenuEntry[] = [];
  if (all.length > 1) {
    entries.push({ type: 'label', id: 'arrangements', label: t('workbench.arrangements.label') });
    for (const arrangement of all) {
      entries.push({
        id: arrangement.name,
        name: t(arrangement.title),
        icon: arrangement.name === current ? <Check aria-hidden /> : undefined,
        onSelect: () => onChoose(arrangement.name),
      });
    }
    entries.push({ type: 'separator', id: 'separator' });
  }
  entries.push(...workspaceEntries(t), { type: 'separator', id: 'workspace-reset' });
  entries.push({ id: 'status-items', name: t('statusItems.menu'), onSelect: () => setStatusEditing(true) });
  entries.push({
    id: 'reset',
    label: t('workbench.arrangements.reset'),
    icon: <RotateCcw aria-hidden />,
    onSelect: onReset,
  });

  return (
    <Popover anchor label={t('statusItems.title')} open={statusEditing} onOpenChange={setStatusEditing}
      onCloseAutoFocus={(event) => { event.preventDefault(); anchor.current?.querySelector('button')?.focus(); }} trigger={<span ref={anchor}><Menu
      label={t('workbench.arrangements.label')}
      align="end"
      content={<><DropdownEntries entries={entries} /><ReadingGuideMenuItem /></>}
      trigger={<IconButton size="sm" label={t('workbench.arrangements.label')} icon={<LayoutPanelLeft aria-hidden />} />}
    /></span>}><StatusCustomizer onDismiss={() => setStatusEditing(false)} /></Popover>
  );
}
