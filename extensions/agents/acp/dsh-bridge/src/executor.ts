/**
 * A client of Orch's executor: the environment's shell and files, reached over MCP's Streamable HTTP.
 *
 * Where a session's arrangement is separated, dsh runs on the provider's machine and the environment is somewhere else.
 * Orch lets the program reach it through one address and one token (`exec` in `_meta.orch`); the tools of that endpoint
 * are `read`, `write`, `shell`, `shell_output` and `shell_stop`. Each call is one POST that is answered either as one
 * JSON document or as an event stream whose first `data:` line is the answer.
 */

/** Where the executor is, and who may speak to it. */
export interface ExecutorAddress {
  url: string
  token: string
}

/** The outcome of a command. */
export interface Ran {
  output: string
  /** Null when the process ended without an exit code (it was stopped). */
  exitCode: number | null
}

/** A failure the executor reported, or one of reaching it. */
export class ExecutorError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ExecutorError'
  }
}

/** How long one call may take before it is given up, in milliseconds: a little over the longest wait a tool makes itself. */
export const CALL_TIMEOUT_MS = 70_000

/** How long one wait for a command's output is, in milliseconds; the executor allows at most this. */
export const WAIT_STEP_MS = 25_000

/** The client. */
export class Executor {
  private readonly address: ExecutorAddress
  private readonly callTimeoutMs: number
  private nextId = 1

  /** `callTimeoutMs` is how long a call may take unless the caller says otherwise (a foreground command is let run longer). */
  constructor(address: ExecutorAddress, callTimeoutMs: number = CALL_TIMEOUT_MS) {
    this.address = address
    this.callTimeoutMs = callTimeoutMs
  }

  /**
   * Calls one tool and returns its structured result, else the JSON its text holds, else the text. `timeoutMs` is how long
   * this call may take, for one that the executor itself lets run longer than the usual (a foreground command).
   */
  async call(tool: string, args: Record<string, unknown>, signal?: AbortSignal, timeoutMs: number = this.callTimeoutMs): Promise<any> {
    const id = this.nextId++
    const body = { jsonrpc: '2.0', id, method: 'tools/call', params: { name: tool, arguments: args } }
    const timeout = AbortSignal.timeout(timeoutMs)
    let response: Response
    try {
      response = await fetch(this.address.url, {
        method: 'POST',
        headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream', authorization: `Bearer ${this.address.token}` },
        body: JSON.stringify(body),
        signal: signal === undefined ? timeout : AbortSignal.any([signal, timeout]),
      })
    } catch (error) {
      throw new ExecutorError(`the executor could not be reached: ${(error as Error).message}`)
    }
    const text = await response.text()
    if (!response.ok) throw new ExecutorError(`the executor answered ${response.status}: ${text.slice(0, 300)}`)
    const streamed = (response.headers.get('content-type') ?? '').startsWith('text/event-stream')
    const candidates = streamed ? text.split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trim()) : [text.trim()]
    for (const candidate of candidates) {
      let message: { id?: unknown; error?: { message?: string }; result?: any }
      try {
        message = JSON.parse(candidate)
      } catch {
        continue
      }
      if (message.id !== id) continue
      if (message.error !== undefined) throw new ExecutorError(message.error.message ?? 'the executor refused the call')
      const result = message.result ?? {}
      const said = (Array.isArray(result.content) ? result.content : []).map((block: { text?: string }) => block.text ?? '').join('\n')
      if (result.isError === true) throw new ExecutorError(said === '' ? `${tool} failed` : said)
      if (result.structuredContent !== undefined && result.structuredContent !== null) return result.structuredContent
      try {
        return JSON.parse(said)
      } catch {
        return said
      }
    }
    throw new ExecutorError("the executor's answer holds no result")
  }

  /** Reads a text file, or a window of its lines. */
  async read(path: string, window: { offset?: number; limit?: number } = {}, signal?: AbortSignal): Promise<string> {
    const answer = await this.call('read', { path, ...window }, signal)
    const text = typeof answer === 'string' ? answer : (answer?.content ?? answer?.text)
    if (typeof text !== 'string') throw new ExecutorError("the executor's answer to a read holds no text")
    return text
  }

  /** Writes a text file. */
  async write(path: string, content: string, signal?: AbortSignal): Promise<void> {
    await this.call('write', { path, content }, signal)
  }

  /** Starts a command in the background and returns the id of the process. */
  async start(command: string, cwd: string | undefined, signal?: AbortSignal): Promise<string> {
    const answer = await this.call('shell', { command, ...(cwd === undefined || cwd === '' ? {} : { cwd }), background: true }, signal)
    const id: unknown = answer?.process_id
    if (typeof id !== 'string') throw new ExecutorError('the executor started no process')
    return id
  }

  /** What a process printed since the last read, waiting up to `waitMs` for more; `running` says whether it goes on. */
  async output(processId: string, waitMs: number, signal?: AbortSignal): Promise<{ output: string; running: boolean; exitCode: number | null }> {
    const answer = await this.call('shell_output', { process_id: processId, wait_ms: waitMs }, signal)
    return {
      output: typeof answer?.output === 'string' ? answer.output : '',
      running: answer?.running !== false,
      exitCode: typeof answer?.exit_code === 'number' ? answer.exit_code : null,
    }
  }

  /** Stops a process. */
  async stop(processId: string, signal?: AbortSignal): Promise<number | null> {
    const answer = await this.call('shell_stop', { process_id: processId }, signal)
    return typeof answer?.exit_code === 'number' ? answer.exit_code : null
  }

  /** Runs a command to its end and returns what it printed and how it ended; a stop of the signal stops the process. */
  async run(command: string, cwd: string | undefined, signal?: AbortSignal): Promise<Ran> {
    const id = await this.start(command, cwd, signal)
    let text = ''
    const stopper = (): void => void this.stop(id).catch(() => undefined)
    signal?.addEventListener('abort', stopper, { once: true })
    try {
      for (;;) {
        const step = await this.output(id, WAIT_STEP_MS, signal)
        text += step.output
        if (!step.running) return { output: text, exitCode: step.exitCode }
      }
    } finally {
      signal?.removeEventListener('abort', stopper)
    }
  }
}
