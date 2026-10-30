/**
 * Executor backends against a real dsh, a scripted model and a stand-in executor: the model's commands and files reach
 * the environment (a directory that is not the one dsh works in), and nowhere else.
 */
import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { commandText, globCommand, shellQuote } from '../src/extras/backends.ts'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { FAKE_TOKEN, FakeExecutor } from './support/fake-executor.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

/** The rows of dsh that register the tools the extra registers. */
const SWITCHED_OFF = ['tool-bash', 'tool-fs', 'tool-fs-search', 'tool-jobs', 'tool-working-directory'].map((id) => `- id: ${id}\n  disabled: true\n`).join('')

/** The results the model was given for each call, by call id. */
function results(model: ScriptedModel): Record<string, string> {
  const found: Record<string, string> = {}
  const last = model.chatRequests[model.chatRequests.length - 1]
  for (const message of last?.body.messages ?? []) {
    if (message.role === 'tool') found[message.tool_call_id] = typeof message.content === 'string' ? message.content : JSON.stringify(message.content)
  }
  return found
}

describe('the pieces of the backends', () => {
  it('quotes a word for a POSIX shell', () => {
    assert.equal(shellQuote("it's"), `'it'\\''s'`)
  })

  it('anchors a glob without a slash at any depth, and leaves one with a slash where it stands', () => {
    assert.match(globCommand('*.ts', undefined, 100), /_ '\*\*\/\*\.ts'$/)
    assert.match(globCommand('src/*.ts', '/work', 100), /^cd '\/work' && .* _ 'src\/\*\.ts'$/)
    assert.match(globCommand('*.ts', undefined, 7), /head -n 7/)
  })

  it('shows a finished command with its streams and exit code', () => {
    assert.equal(commandText({ exit_code: 0, stdout: 'hi\n', stderr: '' }), 'hi')
    assert.equal(commandText({ exit_code: 3, stdout: 'a\n', stderr: 'b\n' }), 'a\n[stderr]\nb\n[exit code: 3]')
    assert.equal(commandText({ exit_code: 0, stdout: '', stderr: '' }), '(no output)')
    assert.match(commandText({ exit_code: null, stdout: '', stderr: '', note: 'The command was stopped.' }), /\[The command was stopped\.\]\n\[the command ended without an exit code\]/)
    assert.match(commandText({ exit_code: 0, stdout: 'x', truncated: true, output_file: '/tmp/o' }), /whole of it is in \/tmp\/o in the environment/)
  })
})

describe('executor backends', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let fake: FakeExecutor
  let environment: string
  let plan: ScriptedReply[] = []
  const exec = (): Record<string, unknown> => ({ exec: { url: fake.url, token: FAKE_TOKEN, tool_set: 'plain' } })
  const call = (id: string, name: string, args: Record<string, unknown>): ScriptedReply => ({ toolCalls: [{ id, name, arguments: args }] })

  before(async () => {
    environment = mkdtempSync(path.join(tmpdir(), 'orch-environment-'))
    mkdirSync(path.join(environment, 'sub'))
    fake = await FakeExecutor.start(environment)
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url, bridge: { backends: { enabled: true }, approvals: { mode: 'bypass' } }, extraPatch: SWITCHED_OFF })
  })
  after(async () => {
    await dsh.close()
    await model.close()
    await fake.close()
    rmSync(environment, { recursive: true, force: true })
  })

  it('says it is on, and offers the tools under dsh\'s own names', async () => {
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.executor_backends, { status: 'on' })
    const sessionId = await dsh.newSession(exec())
    plan = [{ text: 'hi' }]
    await dsh.prompt(sessionId, 'hello')
    const names: string[] = model.chatRequests[0]!.body.tools.map((t: any) => t.function.name)
    for (const name of ['bash', 'read', 'write', 'edit', 'glob', 'grep', 'job_output', 'job_kill', 'job_list']) assert.ok(names.includes(name), `${name} is offered`)
    assert.ok(!names.includes('working_directory'), 'the provider\'s own directory is not offered')
  })

  it('runs commands, writes, reads, edits and searches in the environment and never in dsh\'s own directory', async () => {
    const sessionId = await dsh.newSession(exec())
    const base = model.chatRequests.length
    plan = [
      ...Array(base).fill({ text: 'skip' }),
      call('c-bash', 'bash', { description: 'marks the environment', command: 'echo marked > made.txt; pwd' }),
      call('c-write', 'write', { file_path: 'sub/a.txt', content: 'alpha\nbeta\nbeta two\n' }),
      call('c-read', 'read', { file_path: 'sub/a.txt', offset: 2, limit: 1 }),
      call('c-edit', 'edit', { file_path: 'sub/a.txt', old_string: 'alpha', new_string: 'ALPHA' }),
      call('c-edit-twice', 'edit', { file_path: 'sub/a.txt', old_string: 'beta', new_string: 'gamma' }),
      call('c-empty-write', 'write', { file_path: 'sub/empty.txt', content: '' }),
      call('c-empty', 'read', { file_path: 'sub/empty.txt' }),
      call('c-glob', 'glob', { pattern: '*.txt' }),
      call('c-grep', 'grep', { pattern: 'beta', include: '*.txt' }),
      call('c-fail', 'bash', { description: 'fails', command: 'echo oops >&2; exit 4' }),
      { text: 'finished' },
    ]
    const turn = await dsh.prompt(sessionId, 'work in the environment', undefined, 120000)
    assert.equal(turn.stopReason, 'end_turn')
    const seen = results(model)
    assert.match(seen['c-bash']!, new RegExp(environment.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')), 'the command ran in the environment directory')
    assert.equal(readFileSync(path.join(environment, 'made.txt'), 'utf8'), 'marked\n')
    assert.equal(existsSync(path.join(dsh.cwd, 'made.txt')), false, 'nothing was made in dsh\'s own directory')
    assert.equal(existsSync(path.join(dsh.cwd, 'sub')), false)
    assert.match(seen['c-write']!, /Wrote 20 bytes to sub\/a\.txt/)
    assert.match(seen['c-read']!, /beta/)
    assert.doesNotMatch(seen['c-read']!, /alpha/)
    assert.match(seen['c-edit']!, /Replaced 1 occurrence in sub\/a\.txt/)
    assert.equal(readFileSync(path.join(environment, 'sub/a.txt'), 'utf8'), 'ALPHA\nbeta\nbeta two\n')
    assert.match(seen['c-edit-twice']!, /occurs 2 times/, 'the executor refuses an edit that is not unique, and says why')
    assert.match(seen['c-empty']!, /nothing to show/)
    assert.match(seen['c-glob']!, /made\.txt/)
    assert.match(seen['c-glob']!, /sub\/a\.txt/)
    assert.match(seen['c-grep']!, /a\.txt:2: beta/)
    assert.match(seen['c-fail']!, /\[stderr\]\noops/)
    assert.match(seen['c-fail']!, /\[exit code: 4\]$/)
  })

  it('moves a command to the background, lists it, reads it and stops it', async () => {
    const sessionId = await dsh.newSession(exec())
    const base = model.chatRequests.length
    const started = fake.calls.filter((c) => c.tool === 'shell' && c.args.background === true).length
    const job = `p${started + 1}`
    plan = [
      ...Array(base).fill({ text: 'skip' }),
      call('c-start', 'bash', { description: 'long command', command: 'echo first; sleep 60', run_in_background: true }),
      call('c-out', 'job_output', { job_id: job, wait: true, timeout_ms: 5000 }),
      call('c-list', 'job_list', {}),
      call('c-kill', 'job_kill', { job_id: job }),
      call('c-list2', 'job_list', {}),
      { text: 'finished' },
    ]
    await dsh.prompt(sessionId, 'background', undefined, 120000)
    const seen = results(model)
    assert.match(seen['c-start']!, new RegExp(`job ${job}`))
    assert.match(seen['c-out']!, /first/)
    assert.match(seen['c-out']!, /still running/)
    assert.match(seen['c-list']!, new RegExp(`${job}\\s+running`))
    assert.match(seen['c-kill']!, /was stopped/)
    assert.match(seen['c-list2']!, new RegExp(`${job}\\s+ended`))
  })

  it('refuses to run when the session has no executor, and says so', async () => {
    const sessionId = await dsh.newSession()
    const base = model.chatRequests.length
    const before = fake.calls.length
    plan = [...Array(base).fill({ text: 'skip' }), call('c-none', 'bash', { description: 'no environment', command: 'echo hi' }), { text: 'finished' }]
    await dsh.prompt(sessionId, 'try', undefined, 120000)
    assert.match(results(model)['c-none']!, /no executor is named for this session/)
    assert.equal(fake.calls.length, before, 'nothing reached the executor')
  })

  it('refuses a token of another tool set', async () => {
    const sessionId = await dsh.newSession({ exec: { url: fake.url, token: FAKE_TOKEN, tool_set: 'claude-code' } })
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), call('c-set', 'bash', { description: 'wrong set', command: 'echo hi' }), { text: 'finished' }]
    await dsh.prompt(sessionId, 'try', undefined, 120000)
    assert.match(results(model)['c-set']!, /tool set "claude-code"/)
  })

  it('asks before a change as it does for dsh\'s own tools, and does not reach the executor when refused', async () => {
    const sessionId = await dsh.newSession({ ...exec(), approval_mode: 'ask' })
    dsh.onRequest('session/request_permission', (params: any) => ({ outcome: { outcome: 'selected', optionId: params.options.find((o: any) => o.kind === 'reject_once').optionId } }))
    const base = model.chatRequests.length
    const before = fake.calls.length
    plan = [...Array(base).fill({ text: 'skip' }), call('c-ask', 'write', { file_path: 'refused.txt', content: 'no' }), { text: 'finished' }]
    await dsh.prompt(sessionId, 'write it', undefined, 120000)
    assert.match(results(model)['c-ask']!, /rejected tool "write"/)
    assert.equal(fake.calls.length, before, 'a refused write did not reach the executor')
    assert.equal(existsSync(path.join(environment, 'refused.txt')), false)
  })
})

describe('executor backends of a sub-agent', { skip: skipIntegration }, () => {
  it('reach the executor the root session was given', async () => {
    const environment = mkdtempSync(path.join(tmpdir(), 'orch-environment-'))
    const fake = await FakeExecutor.start(environment)
    // The parent and its sub-agent talk to the same model at once: a request is told apart by what it holds.
    const model = await ScriptedModel.start((request): ScriptedReply => {
      const text = JSON.stringify(request.body.messages)
      const answered = request.body.messages.some((m: any) => m.role === 'tool')
      if (text.includes('CHILD-TASK-MARKER')) return answered ? { text: 'child done' } : { toolCalls: [{ id: 'c-child', name: 'bash', arguments: { description: 'the child works', command: 'echo from-child > child.txt' } }] }
      return answered ? { text: 'delegated' } : { toolCalls: [{ id: 'c-parent', name: 'subagent', arguments: { description: 'a child task', prompt: 'CHILD-TASK-MARKER: make the file' } }] }
    })
    const dsh = await Dsh.launch({ modelUrl: model.url, bridge: { backends: { enabled: true }, approvals: { mode: 'bypass' } }, extraPatch: SWITCHED_OFF })
    try {
      await dsh.handshake()
      const sessionId = await dsh.newSession({ exec: { url: fake.url, token: FAKE_TOKEN, tool_set: 'plain' } })
      await dsh.prompt(sessionId, 'delegate', undefined, 120000)
      const deadline = Date.now() + 60000
      while (!existsSync(path.join(environment, 'child.txt')) && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 100))
      assert.equal(readFileSync(path.join(environment, 'child.txt'), 'utf8'), 'from-child\n', "the sub-agent's command ran in the environment of the root session")
      assert.equal(existsSync(path.join(dsh.cwd, 'child.txt')), false)
    } finally {
      await dsh.close()
      await model.close()
      await fake.close()
      rmSync(environment, { recursive: true, force: true })
    }
  })
})

describe('executor backends that cannot start', { skip: skipIntegration }, () => {
  it('stay off when the arrangement is not separated, and leave dsh\'s own tools', async () => {
    const model = await ScriptedModel.start(() => ({ text: 'hi' }))
    const dsh = await Dsh.launch({ modelUrl: model.url })
    try {
      const hello = await dsh.handshake()
      assert.equal(hello.extras.executor_backends.status, 'off')
      assert.equal(hello.extras.executor_backends.reason, 'unsupported_environment')
      const sessionId = await dsh.newSession()
      await dsh.prompt(sessionId, 'hello')
      assert.ok(model.chatRequests[0]!.body.tools.some((t: any) => t.function.name === 'working_directory'))
    } finally {
      await dsh.close()
      await model.close()
    }
  })

  it('go off with the rows to switch off when dsh\'s own tools hold the names', async () => {
    const model = await ScriptedModel.start(() => ({ text: 'hi' }))
    const dsh = await Dsh.launch({ modelUrl: model.url, bridge: { backends: { enabled: true } } })
    try {
      const hello = await dsh.handshake()
      assert.equal(hello.extras.executor_backends.status, 'off')
      assert.equal(hello.extras.executor_backends.reason, 'self_check_failed')
      assert.match(hello.extras.executor_backends.detail, /tool-bash/)
      const sessionId = await dsh.newSession()
      await dsh.prompt(sessionId, 'hello')
      assert.ok(model.chatRequests[0]!.body.tools.some((t: any) => t.function.name === 'bash'), 'dsh\'s own bash is still there')
    } finally {
      await dsh.close()
      await model.close()
    }
  })
})
