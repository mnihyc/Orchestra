import type { TutorialDefinition } from './model';
export const approvalsWalk: TutorialDefinition = {
  id: 'approvals-walk', title: 'helpRelease.guides.approvals', about: 'helpRelease.walks.approvals.about', article: 'how-approvals-work',
  steps: [
    { target: 'parameters', text: 'helpRelease.walks.approvals.one', screen: '/resources/environments', destination: 'thread-parameters', example: 'parameters' },
    { target: 'approval', text: 'helpRelease.walks.approvals.two', screen: '/approvals', example: 'approval' },
    { target: 'approval', text: 'helpRelease.walks.approvals.three', screen: '/approvals', example: 'approval' },
  ],
};
