/**
 * What the plugin takes from dsh. Everything here is dsh's internal plugin API, which is not promised to stay the
 * same between builds, so it is kept in one place: the structural types below name the members the extras use, and
 * the helpers read services of dsh by name at the moment they are needed.
 *
 * Pinned: dsh 0.2.1-rc.2 (the build Orch ships) and 0.2.1-rc.4 (checked by the same conformance cases).
 */
import type { Channel } from './channel.ts'
import type { BridgeConfig } from './config.ts'
import type { ExtraId, OffReason } from './protocol.ts'
import type { SessionBook } from './session-book.ts'

/** The dsh builds the extras were written against and are tested on. */
export const TESTED_HOST_VERSIONS: readonly string[] = ['0.2.1-rc.2', '0.2.1-rc.4']

/** The Cordis context dsh gives a plugin; only the members the plugin uses are named. */
export interface Ctx {
  on(name: string, listener: (...args: any[]) => unknown, options?: boolean | { prepend?: boolean; global?: boolean }): () => boolean
  get(name: string): any
  provide(name: string, value?: unknown): () => void
  effect?(setup: () => () => void, label?: string): () => void
}

/** Why an extra cannot start; thrown from `start`, caught by the registry, which turns the extra off. */
export class ExtraOff extends Error {
  readonly reason: OffReason
  readonly detail: string | undefined
  constructor(reason: OffReason, detail?: string) {
    super(detail ?? reason)
    this.name = 'ExtraOff'
    this.reason = reason
    this.detail = detail
  }
}

/** What an extra is given. */
export interface ExtraHost {
  ctx: Ctx
  channel: Channel
  config: BridgeConfig
  /** What Orch told of each session in `_meta.orch`. */
  book: SessionBook
  /** Reads a service of dsh now; undefined when it is not there. */
  service<T = any>(name: string): T | undefined
  /** Waits for a service to appear, up to `config.serviceWaitMs`. */
  waitService<T = any>(name: string): Promise<T | undefined>
  /** Whether an extra is on at this moment. */
  isOn(extra: ExtraId): boolean
  /** Turns an extra off after it failed while running; tells Orch. */
  failRuntime(extra: ExtraId, error: unknown): void
  /** Runs `fn` for an event; an exception turns the extra off and is not rethrown. */
  guarded<A extends unknown[]>(extra: ExtraId, fn: (...args: A) => void): (...args: A) => void
  /** State shared by the extras of one process, made on first use. */
  shared<T>(key: string, make: () => T): T
  /** One line to the plugin's diagnostics (stderr). */
  log(line: string): void
}

/** An extra: starts once, then lives on listeners and handlers. */
export interface Extra {
  id: ExtraId
  /** Checks what it needs, installs its listeners and handlers; throws `ExtraOff` if it cannot work. */
  start(host: ExtraHost): Promise<void> | void
}

/** Polls `read` until it returns something, or the wait is over. */
export async function waitFor<T>(read: () => T | undefined, ms: number, stepMs = 20): Promise<T | undefined> {
  const deadline = Date.now() + ms
  for (;;) {
    const value = read()
    if (value !== undefined) return value
    if (Date.now() >= deadline) return undefined
    await new Promise((resolve) => setTimeout(resolve, stepMs))
  }
}

/** The version of the dsh build, or null when it cannot be read from inside the process. */
export async function readHostVersion(): Promise<string | null> {
  try {
    const boot = (await import('@deepseek-ai/dsh-app-boot' as string)) as { getDshRuntimeVersion?: () => string }
    const version = boot.getDshRuntimeVersion?.()
    return typeof version === 'string' ? version : null
  } catch {
    return null
  }
}
