import type { TutorialDefinition } from './model';
export const portsWalk: TutorialDefinition = {
  id: 'ports-walk', title: 'helpRelease.guides.ports', about: 'helpRelease.walks.ports.about', article: 'let-agents-open-ports',
  steps: [
    { target: 'ports', text: 'helpRelease.walks.ports.one', screen: '/resources/environments', destination: 'environment-ports', example: 'ports' },
    { target: 'grants', text: 'helpRelease.walks.ports.two', screen: '/resources/environments', destination: 'environment-ports', example: 'grants' },
    { target: 'template', text: 'helpRelease.walks.ports.three', screen: '/resources/templates', example: 'template' },
    { target: 'policy', text: 'helpRelease.walks.ports.four', screen: '/resources/environments', destination: 'environment-settings', example: 'policy' },
  ],
};
