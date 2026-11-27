/** Registers Help as a workbench view. */
import { CircleQuestionMark } from 'lucide-react';

import { lazyPart } from '@/features/files/lazy';
import { registerView, type ViewProps } from '@/features/workbench/views';

const HelpView = lazyPart<ViewProps>(() => import('./HelpView').then((module) => ({ default: module.HelpView })));

/** Registers the Help view under its stable saved-layout identity. */
export function registerHelpView(): void {
  registerView({
    name: 'help',
    title: 'views.help.title',
    about: 'views.help.about',
    icon: CircleQuestionMark,
    screens: 'all',
    area: 'right',
    body: HelpView,
  });
}
