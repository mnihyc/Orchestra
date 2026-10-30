/** One active walk survives screen/view navigation; transient choices and Help docks are not persisted. */
import { create } from 'zustand';
import { tutorialOf } from '@/help/tutorials';
import { boundedStep } from '@/help/tutorials/progress';

interface TutorialRun {
  id: string | null;
  step: number;
  owner: string | null;
  origin: string | null;
  opener: HTMLElement | null;
  picked: { kind: 'thread' | 'environment'; id: string; name: string } | null;
  dock: HTMLElement | null;
}

export const useTutorialRun = create<TutorialRun>(() => ({ id: null, step: 0, owner: null, origin: null, opener: null, picked: null, dock: null }));

const docks = new Set<HTMLDivElement>();

/** Each Help view owns its ref: closing an overlay restores the still-mounted page's layout space. */
export function createTutorialDock(): (element: HTMLDivElement | null) => void {
  let previous: HTMLDivElement | null = null;
  return (element) => {
    if (previous !== null) docks.delete(previous);
    if (element !== null) docks.add(element);
    previous = element;
    const dock = [...docks].at(-1) ?? null;
    if (useTutorialRun.getState().dock !== dock) useTutorialRun.setState({ dock });
  };
}

export function beginTutorial(id: string, step: number, owner: string, origin: string | null, opener: HTMLElement | null): void {
  const tutorial = tutorialOf(id);
  if (tutorial === undefined) return;
  useTutorialRun.setState({ id, step: boundedStep(step, tutorial.steps.length), owner, origin, opener, picked: null });
}

export function leaveTutorial(): void {
  const opener = useTutorialRun.getState().opener;
  useTutorialRun.setState({ id: null, opener: null, picked: null });
  if (opener?.isConnected === true) opener.focus();
}
