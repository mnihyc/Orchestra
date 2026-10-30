import type { TutorialDefinition } from './model';
export const stepIn: TutorialDefinition = {
  id: 'step-in', title: 'settings.interface.help.tutorials.step-in', about: 'helpRelease.walks.stepIn.about', article: 'workflows',
  steps: [{ target: 'conversation', text: 'helpRelease.walks.stepIn.one' }, { target: 'composer', text: 'helpRelease.walks.stepIn.two' }, { target: 'stop', text: 'helpRelease.walks.stepIn.three' }],
};
