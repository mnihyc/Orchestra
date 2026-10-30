/** What an adapter says it supports, read out of the descriptor it declares and from nowhere else: native fork, images, steering, an agent terminal, reporting what a turn is doing, and what it keeps as memory.
 *
 * A descriptor is whatever the adapter reports, as JSON. Nothing here knows an adapter by name: a member
 * the descriptor does not hold is said as not held, never guessed. The memory words are the three
 * kinds of memory an adapter can have and what each means for the agent's notes.
 */
import { reportedHarnessName } from '@/features/resources/agents/descriptor';

/** The words a feature has: it is there, it is not, or it is there in some form. */
export type Support = 'yes' | 'no' | 'unsaid';

/** What an adapter's own terminal is: none, one that can be watched, or one that can be typed into. */
export type TerminalKind = 'none' | 'view' | 'interactive' | 'unsaid';

/** What an adapter keeps from one session to the next. */
export type MemoryKind = 'directory' | 'home' | 'none' | 'unsaid';

export interface Supports {
  fork: Support;
  images: Support;
  steering: Support;
  terminal: TerminalKind;
  phases: Support;
  memory: MemoryKind;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

const flag = (value: unknown): Support => (value === true ? 'yes' : value === false ? 'no' : 'unsaid');

/** Whether an adapter has declared a descriptor at all: one that has run says what it is, one that has not has nothing to read. */
export function hasDescriptor(descriptor: unknown): boolean {
  return isRecord(descriptor) && Object.keys(descriptor).length > 0;
}

/** What a descriptor says its adapter supports. */
export function supportsOf(descriptor: unknown): Supports {
  const capabilities = isRecord(descriptor) && isRecord(descriptor.capabilities) ? descriptor.capabilities : {};
  const terminal = capabilities.agent_terminal;
  const memory = capabilities.memory;
  return {
    fork: flag(capabilities.fork_native),
    images: flag(capabilities.input_images),
    steering: flag(capabilities.steer),
    // An adapter that says nothing of a terminal of its own has none to show.
    terminal: terminal === 'none' || terminal === 'view' || terminal === 'interactive' ? terminal : 'unsaid',
    phases: flag(capabilities.phases),
    memory: memory === 'directory' || memory === 'home' || memory === 'none' ? memory : 'unsaid',
  };
}

/** The name an adapter gives itself in its descriptor, where it gives one. */
export function harnessName(descriptor: unknown): string | null {
  return reportedHarnessName(descriptor);
}

/** The members of a driver patch's JSON as the names of the drivers it patches: `{ docker: {...} }` patches `docker`. */
export function patchedDrivers(patch: Record<string, unknown>): string[] {
  return Object.keys(patch).sort();
}

/** The text of a JSON object that a person typed, or why it is none. */
export function parseObject(text: string): { ok: true; value: Record<string, unknown> } | { ok: false } {
  try {
    const value: unknown = JSON.parse(text.trim() === '' ? '{}' : text);
    return isRecord(value) ? { ok: true, value } : { ok: false };
  } catch {
    return { ok: false };
  }
}
