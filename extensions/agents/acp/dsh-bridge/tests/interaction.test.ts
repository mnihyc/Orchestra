/** Questions, plan mode and approvals against a real dsh and a scripted model. */
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { lastText } from './support/model-helpers.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

const ask = (questions: unknown): ScriptedReply => ({ toolCalls: [{ name: 'ask_user_question', arguments: { questions } }] })

describe('questions', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  let plan: ScriptedReply[] = []
  before(async () => {
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.questions, { status: 'on' })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('puts the model question to Orch and gives the model the answer', async () => {
    plan = [
      ask([{ id: 'file', question: 'Which file?', header: 'File', options: [{ label: 'a.txt', description: 'the first' }, { label: 'b.txt' }] }]),
      { text: 'ok' },
    ]
    dsh.onRequest('_orch/question/ask', () => ({ answers: [{ id: 'file', selected: ['b.txt'] }] }))
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'pick' }] })
    const asked = dsh.of('_orch/question/ask')[0]!.params as any
    assert.equal(asked.sessionId, sessionId)
    assert.equal(asked.questions[0].question, 'Which file?')
    assert.deepEqual(asked.questions[0].options, [{ label: 'a.txt', description: 'the first' }, { label: 'b.txt' }])
    assert.match(lastText(model.chatRequests[1]!, 'tool'), /b\.txt/)
  })

  it('gives a free-text answer to the model as it was typed', async () => {
    plan = [...plan.slice(0, 0), ...Array(model.chatRequests.length).fill({ text: 'skip' }), ask([{ id: 'name', question: 'What name?' }]), { text: 'ok' }]
    dsh.onRequest('_orch/question/ask', () => ({ answers: [{ id: 'name', selected: [], custom: 'Zebra' }] }))
    const before = model.chatRequests.length
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'name it' }] })
    assert.match(lastText(model.chatRequests[before + 1]!, 'tool'), /Zebra/)
  })

  it('withdraws a question that nobody answered when the turn is cancelled', async () => {
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), ask([{ id: 'x', question: 'Wait?' }])]
    dsh.onRequest('_orch/question/ask', () => new Promise(() => undefined))
    const turn = dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'ask me' }] })
    await dsh.waitForMethod('_orch/question/ask', (p) => p.questions[0].id === 'x')
    dsh.notify('session/cancel', { sessionId })
    const result = await turn
    assert.equal(result.stopReason, 'cancelled')
    const closed = (await dsh.waitForMethod('_orch/question/closed')).params as any
    assert.equal(closed.sessionId, sessionId)
    assert.equal(closed.reason, 'cancelled')
  })

  it('tells the model that the person did not answer when Orch declines', async () => {
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), ask([{ id: 'y', question: 'Sure?' }]), { text: 'ok' }]
    dsh.onRequest('_orch/question/ask', () => {
      throw Object.assign(new Error('declined'), { code: -32001 })
    })
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'ask again' }] })
    assert.match(lastText(model.chatRequests[base + 1]!, 'tool'), /did not answer/i)
  })
})

describe('plan mode', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  let plan: ScriptedReply[] = []
  const written = (): string => path.join(dsh.cwd, 'planned.txt')
  before(async () => {
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url, bridge: { approvals: { mode: 'bypass' } } })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.plan_mode, { status: 'on' })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('switches on and reports it, before and during a turn', async () => {
    const result = await dsh.request('_orch/plan/set', { sessionId, active: true })
    assert.equal(result.outcome, 'committed')
    const state = (await dsh.waitForMethod('_orch/plan/state', (p) => p.active === true)).params as any
    assert.equal(state.sessionId, sessionId)
    assert.deepEqual(await dsh.request('_orch/plan/get', { sessionId }), { sessionId, active: true })
  })

  it('refuses a file write and a changing command, lets a read-only command run, and tells the model why', async () => {
    plan = [
      {
        toolCalls: [
          { id: 'c-write', name: 'write', arguments: { file_path: written(), content: 'x' } },
          { id: 'c-rm', name: 'bash', arguments: { command: `touch ${written()}`, description: 'creates a file' } },
          { id: 'c-ls', name: 'bash', arguments: { command: 'ls', description: 'lists' } },
        ],
      },
      { text: 'ok' },
    ]
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'change things' }] })
    assert.equal(existsSync(written()), false, 'nothing was written')
    const refused = dsh.of('_orch/plan/refused').map((m) => m.params as any)
    assert.deepEqual(refused.map((r) => [r.callId, r.tool]).sort(), [['c-rm', 'bash'], ['c-write', 'write']])
    const tools = (model.chatRequests[1]!.body.messages as any[]).filter((m) => m.role === 'tool')
    const byId = Object.fromEntries(tools.map((m) => [m.tool_call_id, String(m.content)]))
    assert.match(byId['c-write']!, /plan mode is on/i)
    assert.match(byId['c-rm']!, /plan mode is on/i)
    assert.doesNotMatch(byId['c-ls']!, /plan mode is on/i)
  })

  it('shows the submitted plan to Orch and keeps planning with the feedback when asked to', async () => {
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ name: 'exit_plan_mode', arguments: { plan: '# Add the file\n\n1. write it\n' } }] }, { text: 'revising' }]
    dsh.onRequest('_orch/plan/review', () => ({ decision: 'keep_planning', feedback: 'also add a test' }))
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'plan it' }] })
    const review = dsh.of('_orch/plan/review')[0]!.params as any
    assert.equal(review.sessionId, sessionId)
    assert.match(review.plan, /^# Add the file/)
    assert.match(lastText(model.chatRequests[base + 1]!, 'tool'), /also add a test/)
    assert.equal(((await dsh.request('_orch/plan/get', { sessionId })) as any).active, true)
  })

  it('leaves plan mode when the plan is approved, and then lets the write run', async () => {
    const base = model.chatRequests.length
    plan = [
      ...Array(base).fill({ text: 'skip' }),
      { toolCalls: [{ name: 'exit_plan_mode', arguments: { plan: '# Add the file\n\n1. write it\n' } }] },
      { toolCalls: [{ name: 'write', arguments: { file_path: written(), content: 'now' } }] },
      { text: 'written' },
    ]
    dsh.onRequest('_orch/plan/review', () => ({ decision: 'proceed' }))
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'go' }] })
    assert.equal(existsSync(written()), true)
    assert.equal(((await dsh.request('_orch/plan/get', { sessionId })) as any).active, false)
  })
})

describe('approvals', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  let plan: ScriptedReply[] = []
  let decisions: string[] = []
  before(async () => {
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.approvals, { status: 'on' })
    dsh.onRequest('session/request_permission', (params: any) => {
      const option = params.options.find((o: any) => o.kind === (decisions.shift() ?? 'allow_once'))
      return { outcome: { outcome: 'selected', optionId: option.optionId } }
    })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('asks before a command that changes something, and runs it once allowed', async () => {
    const target = path.join(dsh.cwd, 'asked.txt')
    plan = [{ toolCalls: [{ id: 'c1', name: 'bash', arguments: { command: `touch ${target}`, description: 'creates a file' } }] }, { text: 'ok' }]
    decisions = ['allow_once']
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'make it' }] })
    const asked = dsh.of('session/request_permission')
    assert.equal(asked.length, 1)
    assert.equal((asked[0]!.params as any).toolCall.toolCallId, 'c1')
    assert.equal(existsSync(target), true)
  })

  it('does not run it when the answer is no', async () => {
    const target = path.join(dsh.cwd, 'refused.txt')
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ id: 'c2', name: 'bash', arguments: { command: `touch ${target}`, description: 'creates a file' } }] }, { text: 'ok' }]
    decisions = ['reject_once']
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'make it' }] })
    assert.equal(existsSync(target), false)
    assert.match(lastText(model.chatRequests[base + 1]!, 'tool'), /requires approval/)
  })

  it('does not ask before a command that only reads', async () => {
    const before = dsh.of('session/request_permission').length
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ name: 'bash', arguments: { command: 'ls', description: 'lists' } }] }, { text: 'ok' }]
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'look' }] })
    assert.equal(dsh.of('session/request_permission').length, before)
  })

  it('stops asking when the session is set to bypass, and asks again when set back', async () => {
    assert.deepEqual(await dsh.request('_orch/approval/set', { sessionId, mode: 'bypass' }), { mode: 'bypass' })
    const target = path.join(dsh.cwd, 'bypassed.txt')
    const before = dsh.of('session/request_permission').length
    let base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ name: 'bash', arguments: { command: `touch ${target}`, description: 'creates a file' } }] }, { text: 'ok' }]
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'bypass' }] })
    assert.equal(dsh.of('session/request_permission').length, before)
    assert.equal(existsSync(target), true)
    await dsh.request('_orch/approval/set', { sessionId, mode: 'ask' })
    base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ name: 'bash', arguments: { command: `touch ${target}.2`, description: 'creates a file' } }] }, { text: 'ok' }]
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'ask again' }] })
    assert.equal(dsh.of('session/request_permission').length, before + 1)
  })
})
