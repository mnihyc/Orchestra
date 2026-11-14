/** Ask is available beside every work page and starts docked in the right bar. */
import { ACTIONS } from '@/ui/actions';
import { lazy } from 'react';
import { registerView } from '@/features/workbench/views';
import { registerCommands } from '@/features/finder/commands';
import { openAsk } from './open';

export function registerAskView(): void {
  registerView({ name: 'ask', title: 'ask.title', about: 'ask.about', icon: ACTIONS.ask.Icon, screens: 'all', area: 'right', container: 'ask', singleton: true, window: true,
    body: lazy(() => import('./AskView').then((module) => ({ default: module.AskView }))) });
  registerCommands([{ id: 'ask.open', kind: 'view', title: (t) => t('ask.open'), icon: ACTIONS.ask.Icon, run: openAsk }]);
}
