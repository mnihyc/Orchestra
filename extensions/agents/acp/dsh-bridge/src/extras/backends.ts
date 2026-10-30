/**
 * Executor backends: the agent's commands and files run in Orch's environment.
 *
 * In a separated arrangement dsh runs on the provider's machine and the environment is somewhere else. Orch gives the
 * session one address and one token (`exec` in `_meta.orch`) of its executor, and this extra registers the tools the
 * model uses to reach the environment under the names dsh's own tools have (`bash`, `read`, `write`, `edit`, `glob`,
 * `grep`, `job_output`, `job_kill`, `job_list`), so that approvals, plan mode and the model's habits hold unchanged.
 *
 * Why tools and not dsh's file and shell services: those services are also used by dsh itself (its home, its skills,
 * its spill files), by a ripgrep that runs on the harness machine, and by a permission row that refuses to start without
 * a sandbox of its own; replacing them would move dsh's own work to the wrong machine. The tools are what the model
 * sees, and they alone are re-pointed. `research-backends.md` of the notes has the survey that led here.
 *
 * The launch has to switch off the rows that register the same names: `tool-bash`, `tool-fs`, `tool-fs-search`,
 * `tool-jobs` and `tool-working-directory` (whose directory would be the provider's own). When a name is taken the
 * extra goes off and says which.
 */
import { Executor, ExecutorError, type ExecutorAddress } from '../executor.ts'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import { rootSessionId, type Agent } from './shared.ts'

/** The tool set of the executor this extra speaks; Orch asks for a token of this set. */
export const EXECUTOR_TOOL_SET = 'plain'

/** The most a command may be told to run for: the executor's own bound. */
const TIMEOUT_MAX_MS = 3_600_000

/** How long the executor lets a foreground command run when it is not told. */
const FOREGROUND_DEFAULT_MS = 120_000

/** How much longer than the command's own timeout a call waits for the executor's answer, in milliseconds. */
const CALL_SLACK_MS = 30_000

/** How long a `glob` may take before it is stopped; the same as dsh's own. */
const GLOB_TIMEOUT_MS = 30_000

/** The longest the executor waits for output in one call. */
const OUTPUT_WAIT_MAX_MS = 30_000

/** A tool of dsh as far as this extra registers one. */
interface ToolDefinition {
  name: string
  description: string
  parameters: Record<string, unknown>
  output: { schema: Record<string, unknown>; render(args: unknown, value: { text: string }): Array<{ type: 'text'; text: string }> }
  execute(args: unknown, exec: ToolRun): Promise<unknown>
}

/** The call a tool is given. */
interface ToolRun {
  agent?: Agent
  signal: AbortSignal
}

interface ToolsService {
  register(definition: ToolDefinition): () => void
}

/** What a command that was moved to the background is known by. */
interface Job {
  command: string
  running: boolean
  exitCode: number | null
}

type Args = Record<string, unknown>

function str(args: Args, key: string, required: true): string
function str(args: Args, key: string, required?: false): string | undefined
function str(args: Args, key: string, required = false): string | undefined {
  const value = args[key]
  if (value === undefined || value === null) {
    if (required) throw new ExecutorError(`${key} is required`)
    return undefined
  }
  if (typeof value !== 'string') throw new ExecutorError(`${key} must be a string`)
  if (required && value.length === 0) throw new ExecutorError(`${key} must not be empty`)
  return value
}

function num(args: Args, key: string): number | undefined {
  const value = args[key]
  if (value === undefined || value === null) return undefined
  if (typeof value !== 'number' || !Number.isFinite(value)) throw new ExecutorError(`${key} must be a number`)
  return value
}

/** Quotes text as one word of a POSIX shell. */
export function shellQuote(text: string): string {
  return `'${text.replaceAll("'", `'\\''`)}'`
}

/**
 * The command that lists the files (not directories) a glob matches, newest first, at most `max`. A pattern without a
 * `/` matches the base name at any depth, as dsh's own `glob` does; bash's own globbing does the matching, so a
 * `**` descends and a leading dot is matched.
 */
export function globCommand(pattern: string, directory: string | undefined, max: number): string {
  const anchored = pattern.includes('/') ? pattern : `**/${pattern}`
  const body =
    'shopt -s globstar nullglob dotglob; IFS=; files=(); for f in $1; do [ -f "$f" ] && files+=("$f"); done; ' +
    `[ \${#files[@]} -gt 0 ] && ls -1td -- "\${files[@]}" | head -n ${Math.max(1, Math.floor(max))}; true`
  const enter = directory === undefined ? '' : `cd ${shellQuote(directory)} && `
  return `${enter}bash -c ${shellQuote(body)} _ ${shellQuote(anchored)}`
}

/** What a finished command is shown as. */
export function commandText(done: { exit_code?: number | null; stdout?: string; stderr?: string; truncated?: boolean; output_file?: string; note?: string }): string {
  const parts: string[] = []
  if (typeof done.stdout === 'string' && done.stdout.length > 0) parts.push(done.stdout.replace(/\n$/, ''))
  if (typeof done.stderr === 'string' && done.stderr.length > 0) parts.push(`[stderr]\n${done.stderr.replace(/\n$/, '')}`)
  if (done.truncated === true) {
    parts.push(done.output_file === undefined ? '[output was cut in the middle]' : `[output was cut in the middle; the whole of it is in ${done.output_file} in the environment]`)
  }
  if (typeof done.note === 'string' && done.note.length > 0) parts.push(`[${done.note}]`)
  // The marker is the one dsh's own bash ends a failed result with: Orch's adapter reads the exit code from it (`command_exit_marker`).
  if (done.exit_code !== 0) parts.push(done.exit_code === null || done.exit_code === undefined ? '[the command ended without an exit code]' : `[exit code: ${done.exit_code}]`)
  return parts.length === 0 ? '(no output)' : parts.join('\n')
}

const OUTPUT = { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] }

function define(
  name: string,
  description: string,
  properties: Record<string, unknown>,
  required: string[],
  execute: (args: Args, exec: ToolRun) => Promise<string>,
): ToolDefinition {
  return {
    name,
    description,
    // An empty `required` is left out: some providers refuse a schema that has one.
    parameters: { type: 'object', properties, ...(required.length === 0 ? {} : { required }) },
    output: { schema: OUTPUT, render: (_args, value) => [{ type: 'text', text: value.text }] },
    execute: async (args, exec) => ({ text: await execute((args ?? {}) as Args, exec) }),
  }
}

/** The extra. */
export const backends: Extra = {
  id: 'executor_backends',
  async start(host: ExtraHost) {
    const config = host.config.backends
    if (!config.enabled) throw new ExtraOff('unsupported_environment', 'the arrangement is not separated: dsh runs the commands and files itself')
    const tools = await host.waitService<ToolsService>('tools')
    if (tools === undefined || typeof tools.register !== 'function') throw new ExtraOff('host_api_missing', 'the tools service is not loaded')

    const executors = new Map<string, Executor>()
    const jobs = new Map<string, Job>()

    /** The executor of the session a call runs for: what Orch told of the root session, else the plugin's own setting. */
    const executorOf = (exec: ToolRun): Executor => {
      const root = exec.agent === undefined ? undefined : rootSessionId(host, exec.agent.session)
      const told = root === undefined ? undefined : host.book.metaOf(root)?.exec
      let address: ExecutorAddress | undefined
      if (told !== undefined) {
        if (told.tool_set !== undefined && told.tool_set !== EXECUTOR_TOOL_SET) {
          throw new ExecutorError(`the executor was given for the tool set "${told.tool_set}", and this plugin speaks "${EXECUTOR_TOOL_SET}"`)
        }
        address = { url: told.url, token: told.token }
      } else if (config.url !== null) {
        const token = process.env[config.tokenEnv]
        if (token !== undefined && token.length > 0) address = { url: config.url, token }
      }
      if (address === undefined) throw new ExecutorError('no executor is named for this session: the commands and files of the environment are out of reach')
      const key = `${address.url}\u0000${address.token}`
      let found = executors.get(key)
      if (found === undefined) {
        found = new Executor(address)
        executors.set(key, found)
      }
      return found
    }

    const definitions: ToolDefinition[] = [
      define(
        'bash',
        'Run a shell command in the environment and wait for it. The answer has the standard output, the standard error and the exit code. ' +
          'Each call runs in a fresh shell; pass `workdir` instead of using `cd`. A command is stopped when `timeoutMs` has passed ' +
          `(at most ${TIMEOUT_MAX_MS}); anything longer belongs in the background with \`run_in_background\`, which answers at once with a job id ` +
          '(collect with job_output, stop with job_kill). Long output is cut in the middle. Provide `description` before `command` in the arguments.',
        {
          description: { type: 'string', description: 'Clear, concise description of what this command does in active voice, 5-10 words (shown in the UI).' },
          command: { type: 'string', description: 'The command, as text for the environment\'s shell.' },
          timeoutMs: { type: 'number', description: 'Timeout in milliseconds; the command is stopped when it has passed. 120000 when left out.' },
          workdir: { type: 'string', description: "Working directory for this command. Defaults to the session's directory; a relative path is resolved against it." },
          run_in_background: { type: 'boolean', description: 'Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies.' },
        },
        ['description', 'command'],
        async (args, exec) => {
          const command = str(args, 'command', true)
          const workdir = str(args, 'workdir')
          const timeout = num(args, 'timeoutMs')
          if (timeout !== undefined && (timeout < 1 || timeout > TIMEOUT_MAX_MS)) {
            throw new ExecutorError(`timeoutMs is from 1 to ${TIMEOUT_MAX_MS}, and ${timeout} is not; put a longer command in the background`)
          }
          const executor = executorOf(exec)
          if (args['run_in_background'] === true) {
            const id = await executor.start(command, workdir, exec.signal)
            jobs.set(id, { command, running: true, exitCode: null })
            return `Started in the background as job ${id}. Collect its output with job_output, stop it with job_kill.`
          }
          // The executor stops the command at its timeout (120000 ms unless told); the call is let wait that long and a little more.
          const done = await executor.call(
            'shell',
            { command, ...(workdir === undefined ? {} : { cwd: workdir }), ...(timeout === undefined ? {} : { timeout_ms: Math.floor(timeout) }) },
            exec.signal,
            (timeout === undefined ? FOREGROUND_DEFAULT_MS : timeout) + CALL_SLACK_MS,
          )
          return commandText(typeof done === 'object' && done !== null ? done : { stdout: String(done), exit_code: 0 })
        },
      ),
      define(
        'job_output',
        'Read a background job: what it printed since the previous read, and its exit code once it has ended.',
        {
          job_id: { type: 'string', description: 'Job id returned by bash with run_in_background.' },
          wait: { type: 'boolean', description: 'Wait for new output or for the end of the job. Defaults to false.' },
          timeout_ms: { type: 'number', description: 'Longest wait in milliseconds with wait: true.' },
        },
        ['job_id'],
        async (args, exec) => {
          const id = str(args, 'job_id', true)
          const wait = args['wait'] === true ? Math.min(num(args, 'timeout_ms') ?? config.jobWaitMs, OUTPUT_WAIT_MAX_MS) : 0
          const step = await executorOf(exec).output(id, Math.max(0, Math.floor(wait)), exec.signal)
          const job = jobs.get(id)
          if (job !== undefined) {
            job.running = step.running
            job.exitCode = step.exitCode
          }
          const lines = [step.output.length > 0 ? step.output.replace(/\n$/, '') : '(no new output)']
          lines.push(step.running ? '[still running]' : `[ended with exit code ${step.exitCode ?? 'none'}]`)
          return lines.join('\n')
        },
      ),
      define(
        'job_kill',
        'Stop a running background job.',
        { job_id: { type: 'string', description: 'Job id returned by bash with run_in_background.' }, reason: { type: 'string', description: 'Optional short reason.' } },
        ['job_id'],
        async (args, exec) => {
          const id = str(args, 'job_id', true)
          const code = await executorOf(exec).stop(id, exec.signal)
          const job = jobs.get(id)
          if (job !== undefined) {
            job.running = false
            job.exitCode = code
          }
          return `Job ${id} was stopped${code === null ? '' : ` (exit code ${code})`}.`
        },
      ),
      define('job_list', 'List your background jobs of this session with their ids and states.', {}, [], async () => {
        if (jobs.size === 0) return 'No background jobs.'
        return [...jobs.entries()]
          .map(([id, job]) => `${id}  ${job.running ? 'running' : `ended (exit code ${job.exitCode ?? 'none'})`}  ${job.command.length > 80 ? `${job.command.slice(0, 77)}...` : job.command}`)
          .join('\n')
      }),
      define(
        'read',
        'Read a UTF-8 text file of the environment and return line-numbered content.',
        {
          file_path: { type: 'string', description: 'Path to read: absolute, or relative to the session directory.' },
          offset: { type: 'number', description: '1-based first line to return. Defaults to 1.' },
          limit: { type: 'number', description: 'Maximum number of lines to return. Defaults to 2000.' },
        },
        ['file_path'],
        async (args, exec) => {
          const offset = num(args, 'offset')
          const limit = num(args, 'limit')
          const text = await executorOf(exec).read(
            str(args, 'file_path', true),
            { ...(offset === undefined ? {} : { offset: Math.floor(offset) }), ...(limit === undefined ? {} : { limit: Math.floor(limit) }) },
            exec.signal,
          )
          // An image comes back from the executor as an image block, which this tool does not carry on: say so instead of nothing.
          return text === '' ? '(nothing to show: the file has no text, or it is an image, which this tool does not show)' : text
        },
      ),
      define(
        'write',
        'Write a file of the environment with the given content, replacing it if it exists. Parent directories are created.',
        {
          file_path: { type: 'string', description: 'Path to write: absolute, or relative to the session directory.' },
          content: { type: 'string', description: 'The whole content of the file.' },
        },
        ['file_path', 'content'],
        async (args, exec) => {
          const target = str(args, 'file_path', true)
          const content = args['content']
          if (typeof content !== 'string') throw new ExecutorError('content must be a string')
          await executorOf(exec).write(target, content, exec.signal)
          return `Wrote ${Buffer.byteLength(content, 'utf8')} bytes to ${target}.`
        },
      ),
      define(
        'edit',
        'Edit an existing UTF-8 text file of the environment by replacing literal text.',
        {
          file_path: { type: 'string', description: 'Path to edit. Provide `file_path` before `old_string` and `new_string` in the arguments.' },
          old_string: { type: 'string', description: 'Literal text to replace.' },
          new_string: { type: 'string', description: 'Literal replacement text. Use an empty string to delete the match.' },
          replace_all: { type: 'boolean', description: 'Replace all matches. Defaults to false; when false, old_string must appear exactly once.' },
        },
        ['file_path', 'old_string', 'new_string'],
        async (args, exec) => {
          const target = str(args, 'file_path', true)
          const oldText = str(args, 'old_string', true)
          const newText = args['new_string']
          if (typeof newText !== 'string') throw new ExecutorError('new_string must be a string')
          const answer = await executorOf(exec).call('edit', { path: target, old: oldText, new: newText, all: args['replace_all'] === true }, exec.signal)
          const replaced = typeof answer === 'object' && answer !== null && typeof answer.replaced === 'number' ? answer.replaced : undefined
          return replaced === undefined ? `Edited ${target}.` : `Replaced ${replaced} occurrence${replaced === 1 ? '' : 's'} in ${target}.`
        },
      ),
      define(
        'glob',
        'Find files, not directories, of the environment whose paths match a glob pattern, including hidden files. ' +
          `Returns up to ${config.globMax} paths, newest first.`,
        {
          pattern: {
            type: 'string',
            description: 'Glob pattern to match file paths against (e.g. "**/*.ts", "src/**/*.test.js"). A pattern with no "/" matches the base name at any depth; include a separator to anchor the depth.',
          },
          path: { type: 'string', description: "Directory to search in. Defaults to the session's directory; a relative path resolves against it." },
        },
        ['pattern'],
        async (args, exec) => {
          const pattern = str(args, 'pattern', true)
          const executor = executorOf(exec)
          const done = await executor.call('shell', { command: globCommand(pattern, str(args, 'path'), config.globMax), timeout_ms: GLOB_TIMEOUT_MS }, exec.signal)
          if (typeof done?.exit_code === 'number' && done.exit_code !== 0) throw new ExecutorError(commandText(done))
          const text = typeof done?.stdout === 'string' ? done.stdout.trim() : ''
          return text.length === 0 ? 'No files match.' : text
        },
      ),
      define(
        'grep',
        'Search file contents of the environment with a regular expression. Returns matching lines as `path:line: text`.',
        {
          pattern: { type: 'string', description: 'Regular expression to search for.' },
          path: { type: 'string', description: "File or directory to search. Defaults to the session's directory; a relative path resolves against it." },
          include: { type: 'string', description: 'One glob filter for which files to search (e.g. "*.ts").' },
        },
        ['pattern'],
        async (args, exec) => {
          const path = str(args, 'path')
          const include = str(args, 'include')
          const answer = await executorOf(exec).call(
            'search',
            { pattern: str(args, 'pattern', true), ...(path === undefined ? {} : { path }), ...(include === undefined ? {} : { glob: include }), max_results: Math.max(1, Math.floor(config.grepMax)) },
            exec.signal,
          )
          const text = typeof answer === 'string' ? answer : JSON.stringify(answer)
          return text.length === 0 ? 'No matches.' : text
        },
      ),
    ]

    const disposers: Array<() => void> = []
    try {
      for (const definition of definitions) disposers.push(tools.register(definition))
    } catch (error) {
      for (const dispose of disposers.splice(0)) dispose()
      const reason = (error as Error).message
      throw new ExtraOff('self_check_failed', `${reason}; the launch has to switch off tool-bash, tool-fs, tool-fs-search, tool-jobs and tool-working-directory`)
    }
    host.log(`executor backends: ${definitions.map((d) => d.name).join(', ')} reach the environment`)
  },
}
