/** Task guides use the same identities in their screens, catalogue and content checks. */
export const TASK_GUIDES = [
  { id: 'connect-a-machine', title: 'taskGuides.connectMachine' },
  { id: 'add-a-provider', title: 'taskGuides.addProvider' },
  { id: 'register-your-program', title: 'taskGuides.registerProgram' },
  { id: 'temporary-environments', title: 'taskGuides.temporaryEnvironments' },
  { id: 'message-queue', title: 'taskGuides.messageQueue' },
] as const;

export type TaskGuideId = typeof TASK_GUIDES[number]['id'];
