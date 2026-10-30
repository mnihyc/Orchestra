import type { TutorialDefinition } from './model';
export const connectComputer: TutorialDefinition = {
  id: 'connect-a-computer', title: 'settings.interface.help.tutorials.connect-a-computer', about: 'helpRelease.walks.computer.about', article: 'connect-a-machine',
  steps: [{ target: 'machines', text: 'helpRelease.walks.computer.one', screen: '/resources/machines' }, { target: 'installer', text: 'helpRelease.walks.computer.two' }, { target: 'machine-policy', text: 'helpRelease.walks.computer.three', screen: '/resources/machines' }],
};
