/** The views there are, each of something in the model: what it is called, what it is of, which screens offer it, what it needs to be shown, and where it starts.
 *
 * A view is a piece of content with a name and an icon: the navigation, a conversation, the files, a
 * chat. The workbench knows none of them. Each part of the application registers its views here, by
 * name, with what the workbench needs to draw a tab for it and to put it somewhere, and the body
 * that draws it. A new registration under a name replaces the previous definition while keeping
 * the stable identity used by saved layouts.
 */
import type { LucideIcon } from 'lucide-react';
import type { ComponentType } from 'react';
import { useSyncExternalStore } from 'react';

import type { AreaId, Tab } from '@/features/workbench/layout';
import type { TranslationKey } from '@/i18n';
import type { ScreenKind } from '@/routes/-access';
import { currentHeading, type ScreenHeading } from '@/shell/screen';
import { placementFor } from '@/features/workbench/placement';
import type { MenuEntry } from '@/ui/Menu';
export { useViewShown } from '@/features/workbench/viewOpening';

/** What the body of a view is given. */
export interface ViewProps {
  /** The tab it is drawn for, with what it shows. */
  tab: Tab;
  /** The area it is in at the moment: a view may draw itself more tightly in a narrow bar. */
  area: AreaId;
}

/** Native view actions and layout choices share a single menu trigger. */
export interface ViewMenuProps extends ViewProps {
  label: string;
  entries: MenuEntry[];
}

export interface ViewDefinition {
  /** Content owns the focused URL; focusing an inspector never replaces that context. */
  role?: 'content' | 'inspector';
  /** The name tabs refer to it by. It never changes: stored layouts hold it. */
  name: string;
  /** Its name in words. */
  title: TranslationKey;
  /** One sentence that says what it shows and what that is for: what its tab's "?" reads. */
  about: TranslationKey;
  icon: LucideIcon;
  /** The kinds of screen that offer it; `all` for a view that belongs to no one thing. */
  screens: readonly ScreenKind[] | 'all';
  /** The area it is put in when it is opened and its reader has not put it elsewhere. */
  area: AreaId;
  /** The named container it starts in. A screen-dependent place is resolved before opening. */
  container?: string | ((screen: ScreenKind) => string);
  /** Its order inside that container. */
  order?: number | ((screen: ScreenKind) => number);
  /** A small live summary; reads the body's existing data, without a separate request. */
  summary?: ComponentType;
  /** Small actions in the section header. */
  actions?: ComponentType<ViewProps>;
  menu?: ComponentType<ViewMenuProps>;
  /** A view's own creation entrance replaces the generic Add-view button in its sidebar header. */
  add?: ComponentType<ViewProps>;
  /** False for a view that is not closed: the screen of the route. */
  closable?: boolean;
  implemented?: boolean;
  /** A target-specific view is offered only when somebody actually opened a target for it. */
  available?: () => boolean;
  /** An explicitly opened target can keep its own context across route changes; its body still checks access. */
  targeted?: () => boolean;
  /** Opening a different target reuses this view's one tab, preserving its position. */
  singleton?: boolean;
  /** Draws it. */
  body: ComponentType<ViewProps>;
}

const views = new Map<string, ViewDefinition>();
const listeners = new Set<() => void>();
let snapshot: readonly ViewDefinition[] = [];

function changed(): void {
  snapshot = [...views.values()];
  for (const listener of listeners) listener();
}

/** Registers a view under its name. What it returns takes it away again, unless another took its place meanwhile. */
export function registerView(definition: ViewDefinition): () => void {
  const registered: ViewDefinition = { ...definition,
    container: definition.container ?? ((screen) => placementFor(definition.name, screen).container),
    order: definition.order ?? ((screen) => placementFor(definition.name, screen).order),
  };
  views.set(definition.name, registered);
  changed();
  return () => {
    if (views.get(definition.name) === registered) {
      views.delete(definition.name);
      changed();
    }
  };
}

export function viewOf(name: string): ViewDefinition | undefined {
  return views.get(name);
}

/** Whether a kind of screen offers a view. */
export function offers(definition: ViewDefinition, screen: ScreenKind, context: ScreenHeading | null = currentHeading()): boolean {
  if (definition.implemented === false) return false;
  if (definition.targeted?.() === true) return true;
  const name = definition.name;
  // A file/difference's neutral URL does not turn its verified workspace into an unrelated page.
  const workspace = context?.target?.kind === 'environment' && context.views?.ready === true;
  const throughThread = workspace && context.path?.at(-1)?.kind === 'thread';
  const workspaceView = workspace && ['files', 'terminal'].includes(name)
    || throughThread && ['git', 'changes'].includes(name);
  if (definition.screens !== 'all' && !definition.screens.includes(screen) && !workspaceView) return false;
  if (context !== null && definition.available?.() === false) return false;
  if (name === 'help') return context?.views?.help !== false;
  if (['screen', 'navigation', 'resource-navigation'].includes(name)) return true;
  if (name === 'search-result') return definition.available?.() !== false;
  const available = context?.views;
  if (available?.ready === false) return false;
  if (name === 'chat' && available?.chat === false) return false;
  if (name === 'links' && available?.links === false) return false;
  // Contextual workspace views need an actual readable workspace, not just the absence of a refusal.
  if (['files', 'changes', 'git'].includes(name) && available?.workspace !== true) return false;
  // A targeted file can retain its own environment across screens; its registration and body check it.
  if (['file', 'difference'].includes(name) && available?.workspace === false) return false;
  if (name === 'terminal' && available?.terminal === false) return false;
  if (['processes', 'resources'].includes(name) && available?.processes === false) return false;
  return true;
}

export function viewContainer(definition: ViewDefinition, screen: ScreenKind): string {
  return typeof definition.container === 'function' ? definition.container(screen) : definition.container ?? placementFor(definition.name, screen).container;
}

export function viewOrder(definition: ViewDefinition, screen: ScreenKind): number {
  return typeof definition.order === 'function' ? definition.order(screen) : definition.order ?? placementFor(definition.name, screen).order;
}

/** The views a kind of screen offers, in the order they were registered. */
export function viewsFor(screen: ScreenKind): ViewDefinition[] {
  return snapshot.filter((definition) => offers(definition, screen));
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Every registered view, for a component that must follow the register as parts of the application arrive. */
export function useViews(): readonly ViewDefinition[] {
  return useSyncExternalStore(subscribe, () => snapshot);
}
