import { firstThread } from './first-thread';
import { stepIn } from './step-in';
import { shareThread } from './share-a-thread';
import { permissionPage } from './permission-page';
import { connectComputer } from './connect-a-computer';
import { addProvider } from './add-a-provider';
import { codeBesideAgent } from './code-beside-an-agent';
import { writeTogether } from './write-together';
import { teamKnowledge } from './what-the-team-knows';
import { usageLimits } from './usage-and-limits';
import { approvalsWalk } from './approvals-walk';
import { portsWalk } from './ports-walk';
import { liveEnvironmentWalk } from './live-environment-walk';
import { accessWalk } from './access-walk';

export const TUTORIALS = [firstThread, stepIn, shareThread, permissionPage, connectComputer, addProvider, codeBesideAgent, writeTogether, teamKnowledge, usageLimits, approvalsWalk, portsWalk, liveEnvironmentWalk, accessWalk] as const;
export type { TutorialDefinition, TutorialStep } from './model';

export function tutorialOf(id: string) { return TUTORIALS.find((one) => one.id === id); }
