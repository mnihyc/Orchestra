import type { TutorialDefinition } from './model';
export const addProvider: TutorialDefinition = {
  id: 'add-a-provider', title: 'settings.interface.help.tutorials.add-a-provider', about: 'helpRelease.walks.provider.about', article: 'resources',
  steps: [{ target: 'providers', text: 'helpRelease.walks.provider.one', screen: '/resources/providers' }, { target: 'provider-form', text: 'helpRelease.walks.provider.two' }, { target: 'provider-models', text: 'helpRelease.walks.provider.three', screen: '/resources/providers' }],
};
