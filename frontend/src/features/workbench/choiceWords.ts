/** Familiar names and quiet descriptions for choices in the workbench, from each view's existing words. */
import type { TFunction } from 'i18next';

import { containerOf } from '@/features/workbench/containers';
import { viewOf, type ViewDefinition } from '@/features/workbench/views';
import type { TranslationKey } from '@/i18n';

export interface ChoiceWords { name: string; description: string }
interface ChoiceKeys { name: TranslationKey; description: TranslationKey }

const VIEW_NAMES = [
  'screen', 'navigation', 'resource-navigation', 'outline', 'history', 'chat', 'documents', 'people',
  'changes', 'files', 'git', 'environment', 'processes', 'resources', 'ports', 'browsers', 'displays',
  'parameters', 'usage', 'knowledge', 'given', 'diagnostics', 'terminal', 'comments', 'help',
  'browser', 'vnc', 'file', 'difference', 'search-result',
] as const;
const VIEW_KEYS = new Map<string, ChoiceKeys>([
  ...VIEW_NAMES.map((name): [string, ChoiceKeys] => [name, {
    name: `workbench.choices.views.${name}.name`, description: `workbench.choices.views.${name}.description`,
  }]),
  ['links', { name: 'workbench.choices.views.links.name', description: 'workbench.choices.views.links.description' }],
]);

const CONTAINER_NAMES = ['navigation', 'resource-navigation', 'conversation', 'team', 'code', 'environment', 'agent', 'document', 'help'] as const;
const CONTAINER_KEYS = new Map<string, ChoiceKeys>(CONTAINER_NAMES.map((name): [string, ChoiceKeys] => [name, {
  name: `workbench.choices.containers.${name}.name`, description: `workbench.choices.containers.${name}.description`,
}]));

/** Extension views retain their contributed name and explanation. */
export function viewChoice(view: ViewDefinition, t: TFunction): ChoiceWords {
  const keys = VIEW_KEYS.get(view.name) ?? { name: view.title, description: view.about };
  return { name: t(keys.name), description: t(keys.description) };
}

/** A moved view can contribute a container before it has a separate container registration. */
export function containerChoice(name: string, t: TFunction): ChoiceWords {
  const keys = CONTAINER_KEYS.get(name);
  if (keys !== undefined) return { name: t(keys.name), description: t(keys.description) };
  const container = containerOf(name);
  if (container !== undefined) return { name: t(container.title), description: t(container.about) };
  const view = viewOf(name);
  return view === undefined ? { name, description: '' } : viewChoice(view, t);
}
