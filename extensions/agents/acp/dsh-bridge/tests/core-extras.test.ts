/** Streaming, usage, steering and follow-up against a real dsh and a scripted model. */
import assert from 'node:assert/strict'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

const bashCall = { name: 'bash', arguments: { command: 'true', description: 'does nothing' } }

function lastUserText(request: { body: any }): string {
  const messages: any[] = request.body.messages
  const last = [...messages].reverse().find((m) => m.role === 'user')
  return typeof last?.content === 'string' ? last.content : JSON.stringify(last?.content)
}

describe('streaming and usage', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  before(async () => {
    model = await ScriptedModel.start(() => ({
      reasoning: 'Thinking it over.',
      text: 'The answer is forty two.',
      chunkSize: 5,
      chunkDelayMs: 15,
      usage: { prompt: 100, completion: 20, cached: 40, reasoning: 5 },
    }))
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.streaming, { status: 'on' })
    assert.deepEqual(hello.extras.usage, { status: 'on' })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('sends the text and the reasoning of a model call as it arrives, before the turn ends', async () => {
    const turn = dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'what is the answer' }] })
    await dsh.waitForMethod('_orch/stream', (p) => p.frames.some((f: any) => f.kind === 'text'))
    const answered = dsh.messages.some((m) => 'result' in m && (m as any).result?.stopReason !== undefined)
    assert.equal(answered, false, 'the first delta arrives while the turn is still running')
    await turn
    const notices = dsh.of('_orch/stream').map((m) => m.params as any)
    const frames = notices.flatMap((n) => n.frames)
    assert.equal(frames[0].kind, 'start')
    assert.equal(frames.filter((f: any) => f.kind === 'text').map((f: any) => f.delta).join(''), 'The answer is forty two.')
    assert.equal(frames.filter((f: any) => f.kind === 'reasoning').map((f: any) => f.delta).join(''), 'Thinking it over.')
    assert.deepEqual(frames[frames.length - 1], { kind: 'end', outcome: 'committed' })
    const seqs = notices.map((n) => n.seq)
    assert.equal(seqs[0], 0)
    let expected = 0
    for (const notice of notices) {
      assert.equal(notice.seq, expected, 'frames are numbered without gaps')
      expected += notice.frames.length
    }
    assert.ok(notices.every((n) => n.sessionId === sessionId && n.attempt === notices[0].attempt && n.turn === 1 && n.step === 1))
  })

  it('tags the committed text and thought updates with the call that streamed them', async () => {
    const attempt = (dsh.of('_orch/stream')[0]?.params as any).attempt
    const updates = dsh.of('session/update').map((m) => (m.params as any).update)
    const text = updates.find((u) => u.sessionUpdate === 'agent_message_chunk')
    const thought = updates.find((u) => u.sessionUpdate === 'agent_thought_chunk')
    assert.equal(text.content.text, 'The answer is forty two.')
    assert.deepEqual(text._meta.orch, { attempt, turn: 1, step: 1 })
    assert.deepEqual(thought._meta.orch, { attempt, turn: 1, step: 1 })
  })

  it('reports the counts the provider gave for the call, with the cached input apart from the fresh input', async () => {
    const notice = (await dsh.waitForMethod('_orch/usage')).params as any
    assert.equal(notice.sessionId, sessionId)
    assert.equal(notice.provider, 'scripted')
    assert.equal(notice.model, 'scripted-model')
    assert.equal(notice.usage.inputTokens + notice.usage.cacheReadTokens, 100)
    assert.equal(notice.usage.cacheReadTokens, 40)
    assert.equal(notice.usage.outputTokens, 20)
    // The pi-ai route folds reasoning tokens into the output count and does not report them apart: absent, never zero.
    assert.equal(notice.usage.reasoningTokens, undefined)
    assert.equal(notice.usage.cacheWriteTokens, undefined)
    assert.equal(notice.attempt, (dsh.of('_orch/stream')[0]?.params as any).attempt)
  })
})

describe('steering and follow-up', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  let release: () => void
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  before(async () => {
    model = await ScriptedModel.start((request, index): ScriptedReply => {
      if (index === 0) return { text: 'Working.', toolCalls: [bashCall], gate, chunkSize: 4 }
      if (index === 1) return { text: `Saw: ${lastUserText(request)}` }
      return { text: `Then: ${lastUserText(request)}` }
    })
    dsh = await Dsh.launch({ modelUrl: model.url })
    await dsh.handshake()
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('gives a steering message to the model at the next step of the running turn', async () => {
    const turn = dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'start' }] })
    await dsh.waitForMethod('_orch/stream', (p) => p.frames.some((f: any) => f.kind === 'text'))
    const result = await dsh.request('_orch/input/steer', { sessionId, messageId: 'm-steer', content: [{ type: 'text', text: 'use the small file' }] })
    assert.equal(result.accepted, true)
    release()
    await turn
    assert.ok(model.chatRequests.length >= 2)
    assert.match(lastUserText(model.chatRequests[1]!), /use the small file/)
    const states = dsh.of('_orch/input/state').map((m) => (m.params as any))
    assert.deepEqual(states.filter((s) => s.messageId === 'm-steer').map((s) => s.state), ['queued', 'claimed'])
    assert.equal(states.find((s) => s.state === 'claimed').turn, 1)
  })

  it('refuses a steering message for a session that is not live and one with an image', async () => {
    await assert.rejects(dsh.request('_orch/input/steer', { sessionId: 'nope', messageId: 'x', content: [{ type: 'text', text: 'a' }] }), (e: Error & { code?: number }) => e.code === -32003)
    await assert.rejects(dsh.request('_orch/input/steer', { sessionId, messageId: 'x', content: [{ type: 'image', mimeType: 'image/png', data: 'AAAA' }] }), (e: Error & { code?: number }) => e.code === -32602)
  })

  it('queues a follow-up that starts the next turn when this one has ended', async () => {
    const before = model.chatRequests.length
    const result = await dsh.request('_orch/input/followup', { sessionId, messageId: 'm-follow', content: [{ type: 'text', text: 'and then this' }] })
    assert.equal(result.accepted, true)
    await dsh.waitForMethod('_orch/input/state', (p) => p.messageId === 'm-follow' && p.state === 'claimed')
    await dsh.waitFor((m) => 'method' in m && m.method === 'session/update' && JSON.stringify((m as any).params).includes('Then: and then this'), 30000, 'the follow-up answer')
    assert.equal(model.chatRequests.length, before + 1)
  })
})
