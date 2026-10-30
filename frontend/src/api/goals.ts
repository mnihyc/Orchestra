/** A thread's goal: set, edited, paused, resumed and cleared, with ordinary thread execution fencing. */
import { api, ok } from '@/api/client';
import { threadAct } from '@/api/threadRead';
import { threadReadTarget, type ThreadReadTarget } from '@/realtime/threadScope';
import type { ThreadGoal, ThreadGoalCreateBody, ThreadGoalUpdateBody } from '@/generated/api-types';

/** Sets a goal in the caller's name; Orch then keeps the thread working on it. */
export function setGoal(target: ThreadReadTarget, body: ThreadGoalCreateBody): Promise<ThreadGoal> {
  const id = threadReadTarget(target).threadId;
  return threadAct(target, () => ok(api.POST('/threads/{id}/goal', { params: { path: { id } }, body })));
}

/** Edits the goal, pauses it, or sets it going again. */
export function updateGoal(target: ThreadReadTarget, body: ThreadGoalUpdateBody): Promise<ThreadGoal> {
  const id = threadReadTarget(target).threadId;
  return threadAct(target, () => ok(api.PATCH('/threads/{id}/goal', { params: { path: { id } }, body })));
}

/** Takes the goal away; what it did stays in the conversation. */
export function clearGoal(target: ThreadReadTarget): Promise<ThreadGoal> {
  const id = threadReadTarget(target).threadId;
  return threadAct(target, () => ok(api.DELETE('/threads/{id}/goal', { params: { path: { id } } })));
}
