/** The starting container and reading order of built-in views, also used when reading an older layout. */
import type { ScreenKind } from '@/routes/-access';

export interface ViewPlacement { container: string; order: number }

const SECTIONS: Readonly<Record<string, readonly string[]>> = {
  navigation: ['navigation'],
  'resource-navigation': ['resource-navigation'],
  conversation: ['outline', 'history', 'screen'],
  team: ['chat', 'documents', 'people'],
  code: ['changes', 'files', 'git'],
  environment: ['environment', 'processes', 'resources', 'ports', 'browsers', 'displays'],
  agent: ['parameters', 'usage', 'knowledge', 'given', 'diagnostics'],
  document: ['comments', 'outline', 'links'],
  help: ['help'],
};

/** Unknown extensions keep a container of their own, rather than disappearing on an upgrade. */
export function placementFor(view: string, screen?: ScreenKind): ViewPlacement {
  if (screen === 'document' && ['comments', 'outline', 'links'].includes(view)) return { container: 'document', order: ['comments', 'outline', 'links'].indexOf(view) };
  if (screen === 'topic' && view === 'files') return { container: 'environment', order: -1 };
  for (const [container, names] of Object.entries(SECTIONS)) {
    const order = names.indexOf(view);
    if (order !== -1) return { container, order };
  }
  return { container: view, order: 0 };
}
