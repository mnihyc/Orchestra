import type { TutorialDefinition } from './model';
export const liveEnvironmentWalk: TutorialDefinition = {
  id: 'live-environment-walk', title: 'helpRelease.guides.liveEnvironment', about: 'helpRelease.walks.live.about', article: 'change-a-running-environment',
  steps: [
    { target: 'environment-settings', text: 'helpRelease.walks.live.one', screen: '/resources/environments', destination: 'environment-settings', example: 'environment-settings' },
    { target: 'network', text: 'helpRelease.walks.live.two', screen: '/resources/environments', destination: 'environment-overview', example: 'network' },
    { target: 'capability', text: 'helpRelease.walks.live.three', screen: '/approvals', example: 'capability' },
  ],
};
