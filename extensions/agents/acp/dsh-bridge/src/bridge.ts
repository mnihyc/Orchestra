/**
 * The bridge: runs the extras, keeps what Orch said of each session, and announces what works.
 *
 * It has no side effects of its own when it is made; `takeover.ts` and `index.ts` wire it to the process.
 */
import { Channel, errorText } from './channel.ts'
import type { BridgeConfig } from './config.ts'
import { ExtraOff, readHostVersion, TESTED_HOST_VERSIONS, waitFor, type Ctx, type Extra, type ExtraHost } from './host.ts'
import {
  EXTRA_IDS,
  isRequest,
  Method,
  PLUGIN_NAME,
  PROTOCOL_VERSION,
  type ExtraId,
  type ExtraState,
  type Hello,
  type RpcMessage,
} from './protocol.ts'
import { SessionBook } from './session-book.ts'

/** The plugin's version, announced in the hello. */
export const PLUGIN_VERSION = '0.1.0'

/** How long start waits for the extras beyond what each extra itself waits, in milliseconds. */
const START_SLACK_MS = 2000

/** Notifications and requests the plugin may send (announced in the hello). */
const EMITS: readonly string[] = [
  Method.hello,
  Method.extraState,
  Method.stream,
  Method.inputState,
  Method.questionAsk,
  Method.questionClosed,
  Method.planState,
  Method.planReview,
  Method.planRefused,
  Method.usage,
  Method.compaction,
  'elicitation/create',
]

/** Runs the extras and answers the hello; one per process. */
export class Bridge {
  readonly channel: Channel
  readonly book = new SessionBook()
  private readonly ctx: Ctx
  private readonly config: BridgeConfig
  private readonly extras: readonly Extra[]
  private readonly log: (line: string) => void
  private readonly states = new Map<ExtraId, ExtraState>()
  private readonly shareds = new Map<string, unknown>()
  private hostVersion: string | null = null
  private ready: Promise<void> = Promise.resolve()
  private helloSent = false
  private initializeId: unknown = undefined
  private initializeAnswered = false

  constructor(ctx: Ctx, config: BridgeConfig, extras: readonly Extra[], channel: Channel, log: (line: string) => void) {
    this.ctx = ctx
    this.config = config
    this.extras = extras
    this.log = log
    this.channel = channel
    this.book.attach(channel)
    channel.handle(Method.hello, () => this.hello())
    channel.tapIncoming((message) => {
      if (isRequest(message) && message.method === 'initialize') this.initializeId = message.id
    })
    channel.afterWrite((message) => this.afterServerWrite(message))
  }

  /** Starts every extra; resolves when each is on or off. */
  start(): Promise<void> {
    this.ready = this.startAll()
    return this.ready
  }

  /** The state of every extra now. */
  hello(): Hello {
    const extras = {} as Record<ExtraId, ExtraState>
    for (const id of EXTRA_IDS) extras[id] = this.states.get(id) ?? { status: 'off', reason: 'self_check_failed', detail: 'it did not start' }
    return {
      protocol: PROTOCOL_VERSION,
      plugin: { name: PLUGIN_NAME, version: PLUGIN_VERSION },
      host: { name: 'dsh', version: this.hostVersion, tested: this.hostVersion !== null && TESTED_HOST_VERSIONS.includes(this.hostVersion) },
      extras,
      methods: this.channel.handledMethods(),
      emits: [...EMITS],
    }
  }

  private async startAll(): Promise<void> {
    this.hostVersion = this.config.hostVersion ?? (await readHostVersion())
    const host = this.makeHost()
    for (const extra of this.extras) {
      if (!this.config.extras[extra.id]) {
        this.states.set(extra.id, { status: 'off', reason: 'disabled_by_settings' })
        continue
      }
      await this.startOne(extra, host)
    }
    this.sendHelloWhenPossible()
  }

  private async startOne(extra: Extra, host: ExtraHost): Promise<void> {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        Promise.resolve(extra.start(host)),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new ExtraOff('self_check_failed', 'it did not finish starting')), this.config.serviceWaitMs + START_SLACK_MS)
        }),
      ])
      this.states.set(extra.id, { status: 'on' })
    } catch (error) {
      const state: ExtraState =
        error instanceof ExtraOff
          ? error.detail === undefined
            ? { status: 'off', reason: error.reason }
            : { status: 'off', reason: error.reason, detail: error.detail }
          : { status: 'off', reason: 'self_check_failed', detail: errorText(error) }
      this.states.set(extra.id, state)
      this.log(`orch-dsh-bridge: ${extra.id} is off: ${state.status === 'off' ? (state.detail ?? state.reason) : ''}`)
    } finally {
      if (timer !== undefined) clearTimeout(timer)
    }
  }

  private makeHost(): ExtraHost {
    const host: ExtraHost = {
      ctx: this.ctx,
      channel: this.channel,
      config: this.config,
      book: this.book,
      service: (name) => this.ctx.get(name) ?? undefined,
      waitService: (name) => waitFor(() => this.ctx.get(name) ?? undefined, this.config.serviceWaitMs),
      isOn: (id) => this.states.get(id)?.status === 'on' || !this.states.has(id),
      failRuntime: (id, error) => {
        if (this.states.get(id)?.status === 'off') return
        const state: ExtraState = { status: 'off', reason: 'runtime_failure', detail: errorText(error) }
        this.states.set(id, state)
        this.log(`orch-dsh-bridge: ${id} is off: ${errorText(error)}`)
        this.channel.notify(Method.extraState, { extra: id, state })
      },
      guarded:
        (id, fn) =>
        (...args) => {
          if (!host.isOn(id)) return
          try {
            fn(...args)
          } catch (error) {
            host.failRuntime(id, error)
          }
        },
      shared: <T>(key: string, make: () => T): T => {
        if (!this.shareds.has(key)) this.shareds.set(key, make())
        return this.shareds.get(key) as T
      },
      log: this.log,
    }
    return host
  }

  private afterServerWrite(message: RpcMessage): void {
    if (this.initializeAnswered || this.initializeId === undefined) return
    if (!isRequest(message) && message.id === this.initializeId) {
      this.initializeAnswered = true
      this.sendHelloWhenPossible()
    }
  }

  private sendHelloWhenPossible(): void {
    if (this.helloSent || !this.initializeAnswered) return
    void this.ready.then(() => {
      if (this.helloSent) return
      this.helloSent = true
      this.channel.notify(Method.hello, this.hello())
    })
  }
}
