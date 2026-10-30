/** A short authored walk. Navigation is to read screens; a step never contains an API call or a credential. */
import type { TranslationKey } from '@/i18n';
import type { TargetId } from '@/help/targets';

export type TutorialDestination = 'thread-parameters' | 'thread-access' | 'environment-ports'
  | 'environment-settings' | 'environment-overview';
export type TutorialExample = 'parameters' | 'approval' | 'ports' | 'grants' | 'template' | 'policy'
  | 'environment-settings' | 'network' | 'capability' | 'permissions' | 'holders';

export interface TutorialStep {
  target: TargetId;
  text: TranslationKey;
  /** Only a specific control's actual click can advance an action step. */
  advanceOn?: 'click';
  /** A fixed read screen; otherwise the reader keeps/opens the real object they are working on. */
  screen?: '/approvals' | '/resources/machines' | '/resources/providers' | '/resources/templates' | '/resources/environments' | '/settings/usage';
  /** Opens a reader-chosen owned object, never an invented or silently selected id. */
  destination?: TutorialDestination;
  /** An authored illustration may be read when no eligible real control exists. */
  example?: TutorialExample;
}
export interface TutorialDefinition {
  id: string;
  title: TranslationKey;
  about: TranslationKey;
  article: string;
  steps: readonly TutorialStep[];
}
