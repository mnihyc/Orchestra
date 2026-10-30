/** Registered places for related views; the rail and both sizes of the workbench use the same names. */
import { BookOpen, Boxes, Code, FileText, FolderTree, LifeBuoy, MessagesSquare, Server, SlidersHorizontal, Users, type LucideIcon } from 'lucide-react';
import { useSyncExternalStore } from 'react';

import type { SideBarId } from '@/features/workbench/layout';
import type { TranslationKey } from '@/i18n';

export interface ContainerDefinition {
  name: string;
  title: TranslationKey;
  icon: LucideIcon;
  about: TranslationKey;
  area: SideBarId;
}

const definitions = new Map<string, ContainerDefinition>();
const listeners = new Set<() => void>();
let snapshot: readonly ContainerDefinition[] = [];

export function registerContainer(definition: ContainerDefinition): () => void {
  definitions.set(definition.name, definition);
  const changed = () => { snapshot = [...definitions.values()]; for (const listener of listeners) listener(); };
  changed();
  return () => { if (definitions.get(definition.name) === definition) { definitions.delete(definition.name); changed(); } };
}

export const containerOf = (name: string): ContainerDefinition | undefined => definitions.get(name);

export function useContainers(): readonly ContainerDefinition[] {
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => listeners.delete(listener); }, () => snapshot);
}

const BUILT_INS = [
  ['navigation', FolderTree, 'left'], ['resource-navigation', Boxes, 'left'],
  ['conversation', MessagesSquare, 'right'], ['team', Users, 'right'], ['code', Code, 'right'],
  ['environment', Server, 'right'], ['agent', SlidersHorizontal, 'right'], ['document', FileText, 'right'], ['help', LifeBuoy, 'right'],
] as const;

for (const [name, icon, area] of BUILT_INS) registerContainer({
  name, icon, area,
  title: name === 'navigation' ? 'views.navigation.title' : name === 'resource-navigation' ? 'navigation.resources.title' : name === 'help' ? 'views.help.title' : `workbench.containers.${name}.title`,
  about: name === 'navigation' ? 'views.navigation.about' : name === 'resource-navigation' ? 'navigation.resources.about' : name === 'help' ? 'views.help.about' : `workbench.containers.${name}.about`,
});

/** A moved extension can use its own view's name even before it contributes a container. */
export const FALLBACK_CONTAINER_ICON = BookOpen;
