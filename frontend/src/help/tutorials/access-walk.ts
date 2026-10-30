import type { TutorialDefinition } from './model';
export const accessWalk: TutorialDefinition = {
  id: 'access-walk', title: 'helpRelease.guides.access', about: 'helpRelease.walks.access.about', article: 'who-can-see-and-do-what',
  steps: [
    { target: 'permissions', text: 'helpRelease.walks.access.one', screen: '/resources/environments', destination: 'thread-access', example: 'permissions' },
    { target: 'holders', text: 'helpRelease.walks.access.two', screen: '/resources/environments', destination: 'thread-access', example: 'holders' },
    { target: 'permissions', text: 'helpRelease.walks.access.three', screen: '/resources/environments', destination: 'thread-access', example: 'permissions' },
  ],
};
