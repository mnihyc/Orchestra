/**
 * A stand-in for Orch's executor (its plain tool set, less `list`): MCP Streamable HTTP with the tools `read`, `write`,
 * `edit`, `search`, `shell`, `shell_output` and `shell_stop`, working in a real directory on this machine. The answers
 * have the shapes the real one's have (a foreground `shell` answers `{exit_code, stdout, stderr}`; `read` counts lines
 * from 1). A test points the plugin's executor client at it.
 */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import path from 'node:path'

interface Process {
  child: ChildProcess
  output: string
  taken: number
  exit: number | null
  done: boolean
  waiters: Array<() => void>
}

/** The token the fake accepts. */
export const FAKE_TOKEN = 'fake-executor-token'

/** The fake executor. */
export class FakeExecutor {
  readonly calls: Array<{ tool: string; args: any }> = []
  private readonly server: Server
  private readonly processes = new Map<string, Process>()
  private counter = 0
  /** The directory relative paths and commands without one start in. */
  directory: string

  private constructor(server: Server, directory: string) {
    this.server = server
    this.directory = directory
  }

  /** Starts on a free port; `directory` is where a command without a `cwd` runs. */
  static async start(directory: string = process.cwd()): Promise<FakeExecutor> {
    const server = createServer()
    const fake = new FakeExecutor(server, directory)
    server.on('request', (req, res) => void fake.handle(req, res))
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject)
      server.listen(Number(process.env['ORCH_FAKE_EXECUTOR_PORT'] ?? 0), '127.0.0.1', resolve)
    })
    return fake
  }

  /** A path as the executor takes it: relative ones start in its directory. */
  private at(file: string): string {
    return path.resolve(this.directory, file)
  }

  /** Where it answers. */
  get url(): string {
    return `http://127.0.0.1:${(this.server.address() as AddressInfo).port}/mcp/exec`
  }

  /** Stops it and what it started. */
  async close(): Promise<void> {
    for (const proc of this.processes.values()) proc.child.kill('SIGKILL')
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }

  private async handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = []
    for await (const chunk of req) chunks.push(chunk as Buffer)
    if (req.headers.authorization !== `Bearer ${FAKE_TOKEN}`) {
      res.writeHead(401).end('no')
      return
    }
    const message = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    const { name, arguments: args } = message.params
    this.calls.push({ tool: name, args })
    let result: unknown
    let isError = false
    try {
      result = await this.tool(name, args)
    } catch (error) {
      isError = true
      result = (error as Error).message
    }
    const content = [{ type: 'text', text: typeof result === 'string' ? result : JSON.stringify(result) }]
    const body = JSON.stringify({ jsonrpc: '2.0', id: message.id, result: isError ? { isError: true, content } : { content, ...(typeof result === 'object' ? { structuredContent: result } : {}) } })
    // Half of the answers come as an event stream, as a real server may do.
    if (this.counter++ % 2 === 0) res.writeHead(200, { 'content-type': 'application/json' }).end(body)
    else res.writeHead(200, { 'content-type': 'text/event-stream' }).end(`event: message\ndata: ${body}\n\n`)
  }

  private async tool(name: string, args: any): Promise<unknown> {
    switch (name) {
      case 'read': {
        const lines = readFileSync(this.at(args.path), 'utf8').split('\n')
        const from = Math.max(0, (args.offset ?? 1) - 1)
        return { content: (args.limit === undefined ? lines.slice(from) : lines.slice(from, from + args.limit)).join('\n') }
      }
      case 'write':
        mkdirSync(path.dirname(this.at(args.path)), { recursive: true })
        writeFileSync(this.at(args.path), args.content)
        return { ok: true }
      case 'edit': {
        const text = readFileSync(this.at(args.path), 'utf8')
        const count = text.split(args.old).length - 1
        if (count === 0) throw new Error('old text was not found')
        if (count > 1 && args.all !== true) throw new Error(`old text occurs ${count} times; give more context or set all`)
        writeFileSync(this.at(args.path), args.all === true ? text.split(args.old).join(args.new) : text.replace(args.old, () => args.new))
        return { replaced: args.all === true ? count : 1 }
      }
      case 'search': {
        const run = spawnSync('grep', ['-rnE', ...(args.glob === undefined ? [] : ['--include', args.glob]), '--', args.pattern, args.path ?? '.'], { encoding: 'utf8', cwd: this.directory })
        return run.stdout.split('\n').filter((line) => line.length > 0).slice(0, args.max_results ?? 200).map((line) => line.replace(/^([^:]*):(\d+):/, '$1:$2: ')).join('\n')
      }
      case 'shell': {
        if (args.background !== true) {
          const run = spawnSync('bash', ['-c', args.command], { cwd: args.cwd ?? this.directory, encoding: 'utf8', timeout: args.timeout_ms })
          return { exit_code: run.status, stdout: run.stdout, stderr: run.stderr, truncated: false }
        }
        const child = spawn('bash', ['-c', args.command], { cwd: args.cwd ?? this.directory })
        const id = `p${this.processes.size + 1}`
        const proc: Process = { child, output: '', taken: 0, exit: null, done: false, waiters: [] }
        const wake = (): void => proc.waiters.splice(0).forEach((fn) => fn())
        child.stdout?.on('data', (d) => ((proc.output += d), wake()))
        child.stderr?.on('data', (d) => ((proc.output += d), wake()))
        child.on('close', (code) => ((proc.done = true), (proc.exit = code), wake()))
        this.processes.set(id, proc)
        return { process_id: id }
      }
      case 'shell_output': {
        const proc = this.processes.get(args.process_id)
        if (proc === undefined) throw new Error('no such process')
        if (!proc.done && proc.output.length === proc.taken) await new Promise<void>((resolve) => { proc.waiters.push(resolve); setTimeout(resolve, args.wait_ms ?? 0) })
        const output = proc.output.slice(proc.taken)
        proc.taken = proc.output.length
        return { output, running: !proc.done, exit_code: proc.exit }
      }
      case 'shell_stop': {
        const proc = this.processes.get(args.process_id)
        if (proc === undefined) throw new Error('no such process')
        proc.child.kill('SIGKILL')
        await new Promise<void>((resolve) => (proc.done ? resolve() : proc.waiters.push(resolve)))
        return { exit_code: proc.exit }
      }
      default:
        throw new Error(`no tool ${name}`)
    }
  }
}
