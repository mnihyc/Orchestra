/**
 * The line channel on dsh's stdio.
 *
 * dsh's ACP server reads protocol lines from its stdin and writes them to its stdout. This module stands between those
 * two streams and the real ones. Lines for the plugin's own methods (`_orch/*`) and the answers to the plugin's own
 * requests are taken out of what the server reads; a few standard methods the plugin answers itself (plan mode's
 * `session/set_mode`) likewise; everything else reaches the server unchanged. Everything the server writes passes the
 * plugin's taps (which may add a member to a line or drop one) and leaves through one ordered writer together with the
 * plugin's own messages, so Orch reads events in the order they happened and no two writes ever meet inside a line.
 */
import { StringDecoder } from 'node:string_decoder'
import { PassThrough, Writable, type Readable } from 'node:stream'
import {
  ErrorCode,
  isOrchMethod,
  isRequest,
  type RpcError,
  type RpcId,
  type RpcMessage,
  type RpcRequest,
  type RpcResponse,
} from './protocol.ts'

/** The id prefix of this plugin's own requests, which cannot collide with the ids dsh's server issues. */
export const OWN_ID_PREFIX = 'orch-'

/** A handler for one request or notification. A thrown `RpcFault` becomes the error response. */
export type Handler = (params: unknown, request: RpcRequest) => unknown | Promise<unknown>

/** A failure with a JSON-RPC code, for handlers to throw. */
export class RpcFault extends Error {
  readonly code: number
  readonly data: unknown
  constructor(code: number, message: string, data?: unknown) {
    super(message)
    this.name = 'RpcFault'
    this.code = code
    this.data = data
  }
}

/** What the channel needs of its output: whole lines, and a signal when a full buffer has drained. */
export interface LineWriter {
  write(line: string): boolean
  once(event: 'drain', listener: () => void): unknown
}

/** Where the channel reads and writes, and where it reports its own trouble. */
export interface ChannelIo {
  input: Readable
  output: LineWriter
  /** Receives one line of diagnostics; the real process sends it to stderr. */
  log: (line: string) => void
}

interface Pending {
  resolve: (value: unknown) => void
  reject: (error: Error) => void
  method: string
  detach: () => void
}

/** The channel; make one per process. */
export class Channel {
  private readonly handlers = new Map<string, Handler>()
  private readonly interceptors = new Map<string, Handler>()
  private readonly pending = new Map<string, Pending>()
  private readonly outgoingTaps: Array<(message: RpcMessage) => RpcMessage | null> = []
  private readonly afterWriteTaps: Array<(message: RpcMessage) => void> = []
  private readonly incomingTaps: Array<(message: RpcMessage) => void> = []
  private readonly closeListeners: Array<() => void> = []
  private nextId = 1
  private closed = false
  private readonly io: ChannelIo

  /** What dsh's server reads as its stdin. */
  readonly serverInput: PassThrough = new PassThrough()
  /** What dsh's server writes to as its stdout. */
  readonly serverOutput: Writable

  constructor(io: ChannelIo) {
    this.io = io
    let held = ''
    const decoder = new StringDecoder('utf8')
    this.serverOutput = new Writable({
      write: (chunk: Buffer | string, _encoding, done) => {
        held += typeof chunk === 'string' ? chunk : decoder.write(chunk)
        let newline = held.indexOf('\n')
        let flushed: Promise<void> | void = undefined
        while (newline >= 0) {
          const line = held.slice(0, newline).trim()
          held = held.slice(newline + 1)
          if (line.length > 0) flushed = this.fromServer(line) ?? flushed
          newline = held.indexOf('\n')
        }
        if (flushed === undefined) done()
        else flushed.then(() => done(), (error: Error) => done(error))
      },
    })
    this.listen()
  }

  /**
   * The same two server-side streams as the message stream dsh's ACP server accepts in its configuration
   * (`stream`), for a profile that hands it over explicitly. Nothing reads `serverInput` until the server first reads
   * this stream, so a server that reads `process.stdin` instead is not robbed of its lines.
   */
  asMessageStream(): { readable: ReadableStream<RpcMessage>; writable: WritableStream<RpcMessage> } {
    const input = this.serverInput
    const decoder = new StringDecoder('utf8')
    let attached = false
    const readable = new ReadableStream<RpcMessage>(
      {
        pull: (controller) =>
          new Promise<void>((resolve) => {
            if (!attached) {
              attached = true
              let held = ''
              input.on('data', (chunk: Buffer | string) => {
                held += typeof chunk === 'string' ? chunk : decoder.write(chunk)
                let newline = held.indexOf('\n')
                while (newline >= 0) {
                  const line = held.slice(0, newline).trim()
                  held = held.slice(newline + 1)
                  if (line.length > 0) controller.enqueue(JSON.parse(line) as RpcMessage)
                  newline = held.indexOf('\n')
                }
              })
              input.once('end', () => controller.close())
            }
            resolve()
          }),
      },
      { highWaterMark: 0 },
    )
    const writable = new WritableStream<RpcMessage>({
      write: (message) =>
        new Promise<void>((resolve, reject) => {
          this.serverOutput.write(`${JSON.stringify(message)}\n`, (error) => (error ? reject(error) : resolve()))
        }),
    })
    return { readable, writable }
  }

  /** Answers the requests and notifications of one `_orch/*` method. */
  handle(method: string, handler: Handler): void {
    this.handlers.set(method, handler)
  }

  /** Answers a standard method itself, so that it never reaches dsh's server. */
  intercept(method: string, handler: Handler): void {
    this.interceptors.set(method, handler)
  }

  /** The `_orch/*` methods with a handler, sorted. */
  handledMethods(): string[] {
    return [...this.handlers.keys()].sort()
  }

  /** Looks at every message that is read for the server, before it is passed on. Must be quick and must not throw. */
  tapIncoming(tap: (message: RpcMessage) => void): void {
    this.incomingTaps.push(tap)
  }

  /**
   * Looks at every message the server writes before it leaves; returns the message, a changed one, or null to drop
   * it. Must be quick and must not throw.
   */
  tapOutgoing(tap: (message: RpcMessage) => RpcMessage | null): void {
    this.outgoingTaps.push(tap)
  }

  /** Called after a message of the server has been written to the wire. */
  afterWrite(tap: (message: RpcMessage) => void): void {
    this.afterWriteTaps.push(tap)
  }

  /** Called once when the input ends. */
  onClose(listener: () => void): void {
    this.closeListeners.push(listener)
  }

  /** Writes a notification. */
  notify(method: string, params: unknown): void {
    this.writeLine({ jsonrpc: '2.0', method, params })
  }

  /** Sends a request and waits for the answer; rejects with `RpcFault` for an error response. */
  request<T>(method: string, params: unknown, options: { signal?: AbortSignal } = {}): Promise<T> {
    if (this.closed) return Promise.reject(new RpcFault(ErrorCode.notNow, 'the channel is closed'))
    const id = `${OWN_ID_PREFIX}${this.nextId++}`
    return new Promise<T>((resolve, reject) => {
      const signal = options.signal
      const onAbort = (): void => {
        this.pending.delete(id)
        // The peer is told that nobody waits for the answer any more (the protocol's own cancellation of a request).
        this.notify('$/cancel_request', { requestId: id })
        reject(new RpcFault(ErrorCode.notNow, `${method} was withdrawn`))
      }
      const detach = (): void => signal?.removeEventListener('abort', onAbort)
      if (signal?.aborted) {
        reject(new RpcFault(ErrorCode.notNow, `${method} was withdrawn`))
        return
      }
      signal?.addEventListener('abort', onAbort, { once: true })
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject, method, detach })
      this.writeLine({ jsonrpc: '2.0', id, method, params })
    })
  }

  /** Whether the input has ended. */
  get isClosed(): boolean {
    return this.closed
  }

  private listen(): void {
    const decoder = new StringDecoder('utf8')
    let buffer = ''
    const onData = (chunk: Buffer | string): void => {
      buffer += typeof chunk === 'string' ? chunk : decoder.write(chunk)
      let newline = buffer.indexOf('\n')
      while (newline >= 0) {
        const line = buffer.slice(0, newline)
        buffer = buffer.slice(newline + 1)
        if (line.trim().length > 0) this.receive(line)
        newline = buffer.indexOf('\n')
      }
    }
    const onEnd = (): void => {
      const rest = buffer + decoder.end()
      buffer = ''
      if (rest.trim().length > 0) this.receive(rest)
      this.close()
    }
    this.io.input.on('data', onData)
    this.io.input.once('end', onEnd)
    this.io.input.once('close', onEnd)
  }

  private receive(line: string): void {
    let message: RpcMessage
    try {
      message = JSON.parse(line) as RpcMessage
    } catch {
      this.io.log(`orch-dsh-bridge: a line of the input is not JSON (${line.length} bytes), skipped`)
      return
    }
    if (typeof message !== 'object' || message === null) {
      this.io.log('orch-dsh-bridge: a line of the input is not an object, skipped')
      return
    }
    if (isRequest(message)) {
      if (isOrchMethod(message.method)) {
        void this.dispatch(this.handlers.get(message.method), message)
        return
      }
      for (const tap of this.incomingTaps) tap(message)
      const interceptor = this.interceptors.get(message.method)
      if (interceptor !== undefined) {
        void this.dispatch(interceptor, message)
        return
      }
    } else if (typeof message.id === 'string' && message.id.startsWith(OWN_ID_PREFIX)) {
      this.settle(message)
      return
    } else {
      for (const tap of this.incomingTaps) tap(message)
    }
    this.serverInput.write(`${line}\n`)
  }

  private settle(response: RpcResponse): void {
    const id = String(response.id)
    const pending = this.pending.get(id)
    if (pending === undefined) return
    this.pending.delete(id)
    pending.detach()
    if (response.error !== undefined) pending.reject(new RpcFault(response.error.code, response.error.message, response.error.data))
    else pending.resolve(response.result)
  }

  private async dispatch(handler: Handler | undefined, request: RpcRequest): Promise<void> {
    const wantsAnswer = request.id !== undefined && request.id !== null
    if (handler === undefined) {
      if (wantsAnswer) this.respondError(request.id as RpcId, ErrorCode.methodNotFound, `${request.method} is not known`)
      return
    }
    try {
      const result = await handler(request.params, request)
      if (wantsAnswer) this.writeLine({ jsonrpc: '2.0', id: request.id as RpcId, result: result ?? {} })
    } catch (error) {
      if (!wantsAnswer) {
        this.io.log(`orch-dsh-bridge: ${request.method} failed: ${errorText(error)}`)
        return
      }
      if (error instanceof RpcFault) this.respondError(request.id as RpcId, error.code, error.message, error.data)
      else this.respondError(request.id as RpcId, ErrorCode.internalError, errorText(error))
    }
  }

  private respondError(id: RpcId, code: number, message: string, data?: unknown): void {
    const error: RpcError = data === undefined ? { code, message } : { code, message, data }
    this.writeLine({ jsonrpc: '2.0', id, error })
  }

  /** A line the server wrote. Returns a promise when the output asks the server to wait. */
  private fromServer(line: string): Promise<void> | void {
    let message: RpcMessage
    try {
      message = JSON.parse(line) as RpcMessage
    } catch {
      // Something other than the server wrote to the stdout of the process: it is not protocol, so it goes to stderr.
      this.io.log(line)
      return undefined
    }
    let out: RpcMessage | null = message
    for (const tap of this.outgoingTaps) {
      if (out === null) return undefined
      out = tap(out)
    }
    if (out === null) return undefined
    const flushed = this.writeLine(out)
    for (const tap of this.afterWriteTaps) tap(out)
    return flushed
  }

  /** Writes one message as one line. Returns a promise only when the output asks the writer to wait. */
  private writeLine(message: RpcMessage): Promise<void> | void {
    const line = `${JSON.stringify(message)}\n`
    if (this.io.output.write(line)) return undefined
    return new Promise<void>((resolve) => {
      this.io.output.once('drain', resolve)
    })
  }

  private close(): void {
    if (this.closed) return
    this.closed = true
    for (const [id, pending] of this.pending) {
      pending.detach()
      pending.reject(new RpcFault(ErrorCode.notNow, `${pending.method} was not answered before the input ended`))
      this.pending.delete(id)
    }
    this.serverInput.end()
    for (const listener of this.closeListeners) {
      try {
        listener()
      } catch (error) {
        this.io.log(`orch-dsh-bridge: a close listener failed: ${errorText(error)}`)
      }
    }
  }
}

/** The text of anything that was thrown. */
export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : JSON.stringify(error)
}
