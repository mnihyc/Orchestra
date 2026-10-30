/**
 * Starts a real dsh (the ACP profile) with the built plugin loaded, wired to a scripted model, and speaks to it as
 * Orch would: JSON-RPC lines on its stdio.
 *
 * Which dsh: `ORCH_DSH_BIN` names the native executable of the runtime wheel, or `ORCH_DSH_NODE_ENTRY` names the
 * `lib/bin.js` of the npm build (run with this process's Node). Without either, the integration cases are skipped.
 */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import type { RpcMessage, RpcRequest, RpcResponse } from '../../src/protocol.ts'

const here = path.dirname(fileURLToPath(import.meta.url))
const packageDir = path.resolve(here, '../..')

/** The command that runs dsh, or null when the environment names none. */
export function dshCommand(): { command: string; args: string[] } | null {
  const bin = process.env['ORCH_DSH_BIN']
  if (bin !== undefined && bin.length > 0) return { command: bin, args: [] }
  const entry = process.env['ORCH_DSH_NODE_ENTRY']
  if (entry !== undefined && entry.length > 0) return { command: process.execPath, args: [entry] }
  return null
}

/** The reason integration cases are skipped, or undefined when they run. */
export const skipIntegration: string | undefined = dshCommand() === null ? 'set ORCH_DSH_BIN or ORCH_DSH_NODE_ENTRY to run against a real dsh' : undefined

/** How long dsh may take to start and answer `initialize`; a busy machine with a cold disk cache needs minutes. */
const STARTUP_TIMEOUT_MS = 180000

/** Returns the bundle built before the tests; test processes never compete to rebuild it. */
export function buildPlugin(): string {
  const bundle = path.join(packageDir, 'dist/orch-dsh-bridge.mjs')
  if (!existsSync(bundle)) throw new Error('build the plugin before running its tests')
  return bundle
}

/** Options of one dsh run. */
export interface LaunchOptions {
  modelUrl: string
  /** The plugin's `config` (see `src/config.ts`). */
  bridge?: Record<string, unknown>
  /** Environment variables to add; `DSH_PERMISSION_MODE` defaults to `danger-full-access`. */
  env?: Record<string, string>
  /** The directory the sessions work in. */
  cwd?: string
  /** Extra rows of the patch, as YAML text, appended after the standard rows. */
  extraPatch?: string
  /** Load this module instead of the built plugin (for a probe that only observes). */
  probe?: string
  /** `stream`: the `acp` row is told to take its stream from the plugin's service, as an earlier launch recipe did. */
  recipe?: 'plain' | 'stream'
  /** Keep the home directory (for a run that restarts dsh on the same sessions). */
  home?: string
}

/** A pending request that dsh made to the client (a permission prompt, a plugin request). */
export type ClientRequestHandler = (params: any, request: RpcRequest) => unknown | Promise<unknown>

/** A running dsh. */
export class Dsh {
  readonly home: string
  readonly cwd: string
  readonly messages: RpcMessage[] = []
  readonly stderr: string[] = []
  private readonly child: ChildProcessWithoutNullStreams
  private readonly waiters = new Set<() => void>()
  private readonly answers = new Map<string | number, (response: RpcResponse) => void>()
  private readonly handlers = new Map<string, ClientRequestHandler>()
  private nextId = 1
  private buffer = ''
  private readonly ownsHome: boolean
  exited: Promise<number | null>

  private constructor(child: ChildProcessWithoutNullStreams, home: string, cwd: string, ownsHome: boolean) {
    this.child = child
    this.home = home
    this.cwd = cwd
    this.ownsHome = ownsHome
    child.stdout.setEncoding('utf8')
    child.stdout.on('data', (chunk: string) => this.onData(chunk))
    child.stderr.setEncoding('utf8')
    child.stderr.on('data', (chunk: string) => this.stderr.push(chunk))
    this.exited = new Promise((resolve) => child.once('exit', (code) => resolve(code)))
  }

  /** Starts dsh. */
  static async launch(options: LaunchOptions): Promise<Dsh> {
    const run = dshCommand()
    if (run === null) throw new Error('no dsh is named')
    const home = options.home ?? mkdtempSync(path.join(tmpdir(), 'orch-dsh-home-'))
    const cwd = options.cwd ?? mkdtempSync(path.join(tmpdir(), 'orch-dsh-work-'))
    mkdirSync(cwd, { recursive: true })
    const patch = path.join(home, 'orch-test.patch.yml')
    mkdirSync(home, { recursive: true })
    writeFileSync(patch, patchText(options.probe ?? buildPlugin(), options))
    const env: Record<string, string> = {
      PATH: process.env['PATH'] ?? '',
      HOME: home,
      DSH_HOME: home,
      DSH_TELEMETRY_DISABLED: '1',
      DSH_PERMISSION_MODE: 'danger-full-access',
      SCRIPTED_KEY: 'test-key',
      ...options.env,
    }
    const child = spawn(run.command, [...run.args, '--profile', 'acp', '--patch', patch], { cwd, env, stdio: 'pipe' })
    return new Dsh(child, home, cwd, options.home === undefined)
  }

  /** Answers requests of dsh (or of the plugin) for one method. */
  onRequest(method: string, handler: ClientRequestHandler): void {
    this.handlers.set(method, handler)
  }

  /** Sends a request and returns its response (an error response rejects). */
  async request<T = any>(method: string, params: unknown, timeoutMs = 30000): Promise<T> {
    const id = this.nextId++
    const response = new Promise<RpcResponse>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${method} was not answered in ${timeoutMs} ms\n${this.tail()}`)), timeoutMs)
      this.answers.set(id, (value) => {
        clearTimeout(timer)
        resolve(value)
      })
    })
    this.write({ jsonrpc: '2.0', id, method, params })
    const answer = await response
    if (answer.error !== undefined) {
      const error = new Error(`${method}: ${answer.error.message}`) as Error & { code?: number }
      error.code = answer.error.code
      throw error
    }
    return answer.result as T
  }

  /** Sends a notification. */
  notify(method: string, params: unknown): void {
    this.write({ jsonrpc: '2.0', method, params })
  }

  /** The notifications and requests dsh and the plugin sent, of one method. */
  of(method: string): RpcRequest[] {
    return this.messages.filter((m): m is RpcRequest => 'method' in m && m.method === method)
  }

  /** Waits until a message passes the test, looking at those already received too. */
  async waitFor(test: (message: RpcMessage) => boolean, timeoutMs = 30000, label = 'a message', since = 0): Promise<RpcMessage> {
    const deadline = Date.now() + timeoutMs
    for (;;) {
      const found = this.messages.find((message, index) => index >= since && test(message))
      if (found !== undefined) return found
      if (Date.now() > deadline) throw new Error(`${label} did not arrive in ${timeoutMs} ms\n${this.tail()}`)
      await new Promise<void>((resolve) => {
        const done = (): void => {
          this.waiters.delete(done)
          clearTimeout(timer)
          resolve()
        }
        const timer = setTimeout(done, 100)
        this.waiters.add(done)
      })
    }
  }

  /** Waits for a notification or request of a method. */
  async waitForMethod(method: string, where: (params: any) => boolean = () => true, timeoutMs = 30000, since = 0): Promise<RpcRequest> {
    return (await this.waitFor((m) => 'method' in m && m.method === method && where((m as RpcRequest).params), timeoutMs, method, since)) as RpcRequest
  }

  /** Starts a session and returns its id; `meta` is what Orch would put in `_meta.orch`. */
  async newSession(meta?: Record<string, unknown>, mcpServers: unknown[] = []): Promise<string> {
    const params: Record<string, unknown> = { cwd: this.cwd, mcpServers }
    if (meta !== undefined) params['_meta'] = { orch: { version: 1, ...meta } }
    const result = await this.request<{ sessionId: string }>('session/new', params)
    return result.sessionId
  }

  /** Sends a prompt of one text; `meta` is what Orch would put in `_meta.orch` of it. */
  async prompt(sessionId: string, text: string, meta?: Record<string, unknown>, timeoutMs = 60000): Promise<{ stopReason: string }> {
    const params: Record<string, unknown> = { sessionId, prompt: [{ type: 'text', text }] }
    if (meta !== undefined) params['_meta'] = { orch: { version: 1, ...meta } }
    return this.request('session/prompt', params, timeoutMs)
  }

  /** The `session/update` updates received, of one kind. */
  updates(kind: string): any[] {
    return this.of('session/update').map((m) => (m.params as any).update).filter((u) => u.sessionUpdate === kind)
  }

  /** The standard handshake: `initialize`, then wait for the plugin's hello. */
  async handshake(): Promise<any> {
    await this.request('initialize', { protocolVersion: 1, clientCapabilities: {} }, STARTUP_TIMEOUT_MS)
    return (await this.waitForMethod('_orch/hello')).params
  }

  /** Ends the process: closes stdin, then kills it if it stays. */
  async close(): Promise<void> {
    this.child.stdin.end()
    const timer = setTimeout(() => this.child.kill('SIGKILL'), 8000)
    await this.exited
    clearTimeout(timer)
    if (this.ownsHome) rmSync(this.home, { recursive: true, force: true })
  }

  /** The last lines dsh wrote to stderr, for a failure message. */
  tail(): string {
    return `stderr: ${this.stderr.join('').split('\n').slice(-12).join('\n')}`
  }

  private write(message: RpcMessage): void {
    this.child.stdin.write(`${JSON.stringify(message)}\n`)
  }

  private onData(chunk: string): void {
    this.buffer += chunk
    let newline = this.buffer.indexOf('\n')
    while (newline >= 0) {
      const line = this.buffer.slice(0, newline).trim()
      this.buffer = this.buffer.slice(newline + 1)
      if (line.length > 0) this.onLine(line)
      newline = this.buffer.indexOf('\n')
    }
  }

  private onLine(line: string): void {
    let message: RpcMessage
    try {
      message = JSON.parse(line) as RpcMessage
    } catch {
      this.stderr.push(`[not protocol] ${line}\n`)
      return
    }
    if ('method' in message) {
      this.messages.push(message)
      if (message.id !== undefined) void this.answerRequest(message)
    } else if (message.id !== null && this.answers.has(message.id)) {
      this.answers.get(message.id)?.(message)
      this.answers.delete(message.id)
      this.messages.push(message)
    } else {
      this.messages.push(message)
    }
    for (const waiter of [...this.waiters]) waiter()
  }

  private async answerRequest(request: RpcRequest): Promise<void> {
    const handler = this.handlers.get(request.method)
    if (handler === undefined) {
      this.write({ jsonrpc: '2.0', id: request.id as string | number, error: { code: -32601, message: `${request.method} is not handled by the test client` } })
      return
    }
    try {
      const result = await handler(request.params, request)
      this.write({ jsonrpc: '2.0', id: request.id as string | number, result: result ?? {} })
    } catch (error) {
      const code = (error as { code?: number }).code ?? -32603
      this.write({ jsonrpc: '2.0', id: request.id as string | number, error: { code, message: String((error as Error).message ?? error) } })
    }
  }
}

function patchText(bundle: string, options: LaunchOptions): string {
  const config = JSON.stringify(options.bridge ?? {})
  const plugin =
    options.probe === undefined
      ? `    - id: orch-bridge
      name: ${JSON.stringify(bundle)}
      config: ${config}
`
      : `    - id: orch-probe
      name: ${JSON.stringify(options.probe)}
`
  return `- insert:
${plugin}    - id: tool-ask-user
      name: '@deepseek-ai/dsh-tool-ask-user'
- id: llm-pi-ai
  config:
    providers:
      scripted:
        apiKeyEnv: SCRIPTED_KEY
        api: openai-completions
        baseURL: ${JSON.stringify(options.modelUrl)}
        compat:
          supportsDeveloperRole: false
          maxTokensField: max_tokens
        models:
          - id: scripted-model
            contextWindow: 131072
            maxTokens: 4096
- id: agent-default-model
  config:
    provider: scripted
    model: scripted-model
- id: acp
${options.recipe === 'stream' ? '  inject:\n    - acpAppStartup\n    - orchBridge\n' : ''}  config:
    provider: scripted
    model: scripted-model
${options.recipe === 'stream' ? '    stream: !!js ctx.orchBridge.stream\n' : ''}- id: session-log-deepseek
  disabled: true
- id: plugin-package-inventory-deepseek
  disabled: true
${options.extraPatch ?? ''}`
}
