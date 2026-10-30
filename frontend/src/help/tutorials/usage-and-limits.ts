import type { TutorialDefinition } from './model';
export const usageLimits: TutorialDefinition = {
  id: 'usage-and-limits', title: 'settings.interface.help.tutorials.usage-and-limits', about: 'helpRelease.walks.usage.about', article: 'settings-and-usage',
  steps: [{ target: 'usage', text: 'helpRelease.walks.usage.one', screen: '/settings/usage' }, { target: 'limits', text: 'helpRelease.walks.usage.two' }, { target: 'thread-usage', text: 'helpRelease.walks.usage.three' }],
};
