/**
 * The scripts of the Rust cases (`extensions/agents/acp/tests/dsh_plugin.rs`) against the plugin in a real dsh, with the
 * repository's own scripted model (`orch-mock-llm`) as its provider and this file's client in the adapter's place.
 *
 * The Rust cases use the mock's script language (`#!mock`, `tool`, `then`, `usage`, `chunk`, `delay`); this file proves
 * those scripts do what the cases assume against dsh and the plugin without a Rust build, and keeps them honest: a change
 * of the mock, of dsh's tool arguments or of the plugin that breaks a script fails here first.
 *
 * Needs `ORCH_MOCK_LLM_BIN`, the built `orch-mock-llm`; without it the cases are skipped.
 */
import assert from 'node:assert/strict'
import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'

const mockBin = process.env['ORCH_MOCK_LLM_BIN']

/** Starts the mock on a free port and says where it answers. */
async function startMock(bin: string): Promise<{ url: string; child: ChildProcess }> {
  const listen = process.env['ORCH_MOCK_LLM_LISTEN']
  const child = spawn(bin, ['--model', 'scripted', ...(listen === undefined ? [] : ['--listen', listen])], { stdio: ['ignore', 'pipe', 'pipe'] })
  const url = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('the mock did not say where it listens')), 20000)
    let text = ''
    child.stdout!.setEncoding('utf8')
    child.stdout!.on('data', (chunk: string) => {
      text += chunk
      const found = /listening on (http:\/\/127\.0\.0\.1:\d+)/.exec(text)
      if (found !== null) {
        clearTimeout(timer)
        resolve(found[1]!)
      }
    })
    child.once('exit', () => reject(new Error('the mock ended before it listened')))
  })
  return { url: `${url}/v1`, child }
}

/** Whether this build of the mock answers Chat Completions (an older build does not have the route). */
async function servesChat(bin: string): Promise<boolean> {
  const started = await startMock(bin)
  try {
    const answer = await fetch(`${started.url}/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer x' },
      body: JSON.stringify({ model: 'scripted', messages: [{ role: 'user', content: 'hi' }] }),
    })
    return answer.status !== 404
  } finally {
    const exited = new Promise<void>((resolve) => started.child.once('exit', () => resolve()))
    started.child.kill('SIGKILL')
    await exited
  }
}

const skip =
  skipIntegration ??
  (mockBin === undefined || mockBin === ''
    ? 'set ORCH_MOCK_LLM_BIN to the built orch-mock-llm'
    : (await servesChat(mockBin))
      ? undefined
      : 'this build of orch-mock-llm has no Chat Completions route: use one built from this tree')

const text = (update: any): string => (update.content?.type === 'text' ? update.content.text : '')

describe('the scripts of the Rust cases', { skip }, () => {
  let mock: { url: string; child: ChildProcess }
  let dsh: Dsh
  let sessionId: string
  let work: string
  before(async () => {
    mock = await startMock(mockBin!)
    dsh = await Dsh.launch({ modelUrl: mock.url })
    await dsh.handshake()
    sessionId = await dsh.newSession({ approval_mode: 'ask' })
    work = mkdtempSync(path.join(tmpdir(), 'orch-mock-work-'))
  })
  after(async () => {
    await dsh.close()
    mock.child.kill('SIGKILL')
    rmSync(work, { recursive: true, force: true })
  })

  const turn = async (script: string, meta?: Record<string, unknown>): Promise<{ stopReason: string }> => {
    const queued = await fetch(`${mock.url.replace(/\/v1$/, '')}/_mock/queue`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ script: script.replace(/^#!mock\n/, '') }),
    })
    assert.equal(queued.status, 200, await queued.text())
    return dsh.prompt(sessionId, 'Run the scripted conformance case.', meta, 120000)
  }
  const finalText = (): string =>
    dsh
      .updates('agent_message_chunk')
      .map(text)
      .join('')

  it('streams the text in pieces and reports the counts of the call with the cache apart', async () => {
    const words = 'The quick brown fox jumps over the lazy dog.'
    const before = dsh.of('_orch/stream').length
    const result = await turn(`#!mock\nusage 1000 20 cached=400\nchunk 4\ndelay 120\nsay ${words}\n`)
    assert.equal(result.stopReason, 'end_turn')
    const frames = dsh.of('_orch/stream').slice(before).flatMap((m) => (m.params as any).frames).filter((f: any) => f.kind === 'text')
    assert.ok(frames.length >= 2, `the text came in ${frames.length} pieces`)
    assert.equal(frames.map((f: any) => f.delta).join(''), words)
    const notice = dsh.of('_orch/usage').map((m) => m.params as any).find((p) => p.usage.outputTokens === 20)
    assert.ok(notice, 'a notice with the counts of the call')
    assert.equal(notice.usage.inputTokens + notice.usage.cacheReadTokens, 1000)
    assert.equal(notice.usage.cacheReadTokens, 400)
  })

  it('puts the question of the model to the person and gives the model the answer', async () => {
    dsh.onRequest('_orch/question/ask', () => ({ answers: [{ id: 'choice', selected: ['Second'] }] }))
    const script =
      '#!mock\ntool ask_user_question {"questions":[{"id":"choice","header":"Choice","question":"Which value?","options":[{"label":"First","description":"First value"},{"label":"Second","description":"Second value"}]}]}\nthen\nsay question finished\n'
    const result = await turn(script)
    assert.equal(result.stopReason, 'end_turn')
    const asked = dsh.of('_orch/question/ask').pop()!.params as any
    assert.equal(asked.questions[0].id, 'choice')
    assert.equal(asked.questions[0].options.length, 2)
    assert.match(finalText(), /question finished/)
  })

  it('asks before a command that changes something, runs it once allowed, and does not ask for one that only reads', async () => {
    const refused = path.join(work, 'refused.txt')
    const allowed = path.join(work, 'allowed.txt')
    const pick = (kind: string) => (params: any) => ({ outcome: { outcome: 'selected', optionId: params.options.find((o: any) => o.kind === kind).optionId } })
    dsh.onRequest('session/request_permission', pick('reject_once'))
    await turn(`#!mock\ntool bash ${JSON.stringify({ command: `touch ${refused}`, description: 'creates a file' })}\nthen\nsay denied\n`)
    assert.equal(dsh.of('session/request_permission').length, 1)
    assert.equal(existsSync(refused), false)
    dsh.onRequest('session/request_permission', pick('allow_once'))
    await turn(`#!mock\ntool bash ${JSON.stringify({ command: `touch ${allowed}`, description: 'creates a file' })}\nthen\nsay done\n`)
    assert.equal(dsh.of('session/request_permission').length, 2)
    assert.equal(existsSync(allowed), true)
    await turn('#!mock\ntool bash {"command":"ls","description":"lists the directory"}\nthen\nsay listed\n')
    assert.equal(dsh.of('session/request_permission').length, 2, 'a command that only reads did not ask')
  })

  it('refuses a change while planning, shows the plan, and ends planning when the person proceeds', async () => {
    const planned = path.join(work, 'planned.txt')
    dsh.onRequest('_orch/plan/review', () => ({ decision: 'proceed' }))
    assert.equal((await dsh.request('_orch/plan/set', { sessionId, active: true })).outcome, 'committed')
    const script = `#!mock\ntool bash ${JSON.stringify({ command: `touch ${planned}`, description: 'creates a file' })}\nthen\ntool exit_plan_mode ${JSON.stringify({ plan: '# Create the file\n\n1. touch it\n' })}\nthen\nsay planned\n`
    const before = dsh.of('session/request_permission').length
    const result = await turn(script)
    assert.equal(result.stopReason, 'end_turn')
    const review = dsh.of('_orch/plan/review').pop()!.params as any
    assert.match(review.plan, /^# Create the file/)
    assert.equal(existsSync(planned), false)
    assert.equal(dsh.of('session/request_permission').length, before, 'the refused change was not even put to the person')
    assert.equal(((await dsh.request('_orch/plan/get', { sessionId })) as any).active, false)
  })

  it('takes a message added while the model writes', async () => {
    const since = dsh.messages.length
    const running = turn('#!mock\nchunk 4\ndelay 150\nsay Working on it, one moment please.\ntool bash {"command":"ls","description":"lists"}\nthen\nsay finished\n')
    await dsh.waitForMethod('_orch/stream', (p) => p.frames.some((f: any) => f.kind === 'text'), 30000, since)
    const answered = await dsh.request('_orch/input/steer', { sessionId, messageId: 'm-mock-steer', content: [{ type: 'text', text: 'Also mention the colour purple' }] })
    assert.equal(answered.accepted, true)
    assert.equal((await running).stopReason, 'end_turn')
    const states = dsh.of('_orch/input/state').map((m) => m.params as any).filter((p) => p.messageId === 'm-mock-steer').map((p) => p.state)
    assert.deepEqual(states, ['queued', 'claimed'], 'the message reached a step of the running turn')
  })

  it('ends a turn that is stopped while the model writes, and reports the call as cut short', async () => {
    const since = dsh.messages.length
    const running = turn('#!mock\nchunk 3\ndelay 400\nsay Once upon a time there was a very long sentence that goes on and on and on.\n')
    await dsh.waitForMethod('_orch/stream', (p) => p.frames.some((f: any) => f.kind === 'text'), 30000, since)
    dsh.notify('session/cancel', { sessionId })
    assert.equal((await running).stopReason, 'cancelled')
    const last = dsh.of('_orch/usage').pop()!.params as any
    assert.equal(last.interrupted, true)
  })
})
