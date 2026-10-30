/** The extras, in the order they start. */
import type { Extra } from '../host.ts'
import { approvals } from './approvals.ts'
import { backends } from './backends.ts'
import { compaction } from './compaction.ts'
import { fork } from './fork.ts'
import { history } from './history.ts'
import { plan } from './plan.ts'
import { questions } from './questions.ts'
import { skills } from './skills.ts'
import { steering } from './steering.ts'
import { streaming } from './streaming.ts'
import { usage } from './usage.ts'

export const EXTRAS: Extra[] = [streaming, steering, questions, plan, approvals, usage, compaction, skills, fork, history, backends]
