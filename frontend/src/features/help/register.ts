/** Registers Help as a workbench view. */
import { CircleQuestionMark } from 'lucide-react';

import { HelpView } from '@/features/help/HelpView';
import { registerView } from '@/features/workbench/views';

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
