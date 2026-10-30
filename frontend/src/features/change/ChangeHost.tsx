/** Draws the change that was asked for with `openChange`: the one place in the frame of the application where the wizard stands.
 *
 * Screens do not draw the wizard; they name a thing and what is to be done to it, and this stands once in the shell and
 * draws the right one: a fork or a switch of a thread, several threads, an environment's update, recreation or move,
 * a provider's runtime, what is idle at a host or a place, or the progress of a job that was started already.
 */
import { type ComponentProps, Suspense } from 'react';

import type { MoveDialog as MoveDialogType } from '@/features/fleet/MoveDialog';
import type { BatchChange as BatchChangeDialog } from '@/features/change/BatchChange';
import type { EnvironmentChange as EnvironmentChangeDialog } from '@/features/change/EnvironmentChange';
import type { Recycle as RecycleDialog } from '@/features/change/Recycle';
import type { ThreadChange as ThreadChangeDialog } from '@/features/change/ThreadChange';
import { useChangeHost } from '@/features/change/target';
import { lazyPart } from '@/features/files/lazy';
import { SideAskHost } from '@/features/side-threads/AskOnSide';
import type { UpdateDialog as UpdatesDialog } from '@/features/fleet/UpdateDialog';

// Each dialog is a part of its own, fetched the first time such a change is asked for: the frame that stands on every
// screen carries none of them.
const ThreadChange = lazyPart<ComponentProps<typeof ThreadChangeDialog>>(() => import('@/features/change/ThreadChange').then((module) => ({ default: module.ThreadChange })));
const BatchChange = lazyPart<ComponentProps<typeof BatchChangeDialog>>(() => import('@/features/change/BatchChange').then((module) => ({ default: module.BatchChange })));
const EnvironmentChange = lazyPart<ComponentProps<typeof EnvironmentChangeDialog>>(() => import('@/features/change/EnvironmentChange').then((module) => ({ default: module.EnvironmentChange })));
const MoveDialog = lazyPart<ComponentProps<typeof MoveDialogType>>(() => import('@/features/fleet/MoveDialog').then((module) => ({ default: module.MoveDialog })));
const Recycle = lazyPart<ComponentProps<typeof RecycleDialog>>(() => import('@/features/change/Recycle').then((module) => ({ default: module.Recycle })));
const Updates = lazyPart<ComponentProps<typeof UpdatesDialog>>(() => import('@/features/fleet/UpdateDialog').then((module) => ({ default: module.UpdateDialog })));

export function ChangeHost() {
  return (
    <>
      <Suspense fallback={null}><Wizard /></Suspense>
      <SideAskHost />
    </>
  );
}

function Wizard() {
  const current = useChangeHost((state) => state.current);
  const open = useChangeHost((state) => state.open);
  const hide = useChangeHost((state) => state.hide);
  if (current === null) return null;
  const { target, options, asked } = current;
  const kind = options.kind;
  if (target.kind === 'provider' && kind === 'update' && open) return <Updates key={asked} targets={{ environments: [], providers: [target.id] }} onClose={hide} />;
  if (target.kind === 'environments' && kind === 'update' && open) return <Updates key={asked} targets={{ environments: target.ids, node: target.node }} onClose={hide} />;

  if ((kind === 'recover' || kind === 'move' || kind === 'progress' && options.batch !== undefined) && target.kind === 'environments') return <MoveDialog key={asked} source={{ ids: target.ids }} batch={options.batch ?? null} open={open} onClose={hide} />;
  if (kind === 'recover' && (target.kind === 'host' || target.kind === 'machine' || target.kind === 'environment')) return <MoveDialog key={asked} source={{ target: { type: target.kind, id: target.id } }} open={open} onClose={hide} />;
  // A thread: one fork or switch, or the progress of the job that holds it.
  if (target.kind === 'thread' && (kind === 'switch' || kind === 'fork' || kind === 'progress')) {
    return <ThreadChange key={asked} threadId={target.id} kind={kind === 'fork' ? 'fork' : 'switch'} toTurn={options.toTurn ?? null} focus={options.focus ?? null} job={options.job ?? null} say={options.say ?? null} open={open} onClose={hide} />;
  }
  // Several threads: from a selection, a topic, a provider or an environment; or the progress of a batch that was started.
  if (kind === 'switch' || (kind === 'progress' && options.batch !== undefined)) {
    const batch = options.batch ?? null;
    if (target.kind === 'threads') return <BatchChange key={asked} source={{ kind: 'threads', threads: target.threads, label: target.from?.name ?? null }} batch={batch} open={open} onClose={hide} />;
    if (target.kind === 'model') return <BatchChange key={asked} source={{kind:'model',providerId:target.providerId,id:target.id}} batch={batch} open={open} onClose={hide} />;
    if (target.kind === 'topic' || target.kind === 'provider' || target.kind === 'agent' || target.kind === 'environment') return <BatchChange key={asked} source={{ kind: target.kind, id: target.id }} batch={batch} open={open} onClose={hide} />;
  }
  // An environment: update, recreate, move, or the progress of its job.
  if (target.kind === 'environment' && (kind === 'update' || kind === 'recreate' || kind === 'move' || kind === 'progress')) {
    return <EnvironmentChange key={asked} environmentId={target.id} kind={kind === 'progress' || kind === 'update' ? 'update' : kind} job={options.job ?? null} open={open} onClose={hide} />;
  }
  // What is idle at a host or at a place, stopped to make room.
  if ((target.kind === 'host' || target.kind === 'node') && kind === 'recycle') {
    return <Recycle key={asked} scope={{ kind: target.kind, id: target.id, name: target.name ?? '' }} open={open} onClose={hide} />;
  }
  return null;
}
