/** People start agent work inside their shared topic, beside its human chat. */
import type { TutorialDefinition } from './model';
export const firstThread: TutorialDefinition = {
  id: 'first-thread', title: 'settings.interface.help.tutorials.first-thread', about: 'helpRelease.walks.firstThread.about', article: 'getting-started',
  steps: [{ target: 'new-thread', text: 'helpRelease.walks.firstThread.one', advanceOn: 'click' }, { target: 'composer', text: 'helpRelease.walks.firstThread.two' }, { target: 'conversation', text: 'helpRelease.walks.firstThread.three' }],
};
