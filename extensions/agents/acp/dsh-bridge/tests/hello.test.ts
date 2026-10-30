/** The handshake: what the plugin announces, and that the plain protocol is untouched by it. */
import assert from 'node:assert/strict'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { ScriptedModel } from './support/scripted-model.ts'

describe('hello', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  before(async () => {
    model = await ScriptedModel.start(() => ({ text: 'ok' }))
    dsh = await Dsh.launch({ modelUrl: model.url })
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('announces the protocol version, the plugin, the host and every extra after initialize', async () => {
    const hello = await dsh.handshake()
    assert.equal(hello.protocol, 1)
    assert.equal(hello.plugin.name, 'orch-dsh-bridge')
    assert.equal(hello.host.name, 'dsh')
    assert.deepEqual(Object.keys(hello.extras).sort(), [
      'approvals', 'compaction', 'executor_backends', 'fork', 'native_carry', 'plan_mode', 'questions', 'skills', 'steering', 'streaming', 'usage',
    ])
  })

  it('answers a method it does not know as method not found and leaves the session protocol working', async () => {
    await assert.rejects(dsh.request('_orch/nothing', {}), (error: Error & { code?: number }) => error.code === -32601)
    const sessionId = await dsh.newSession()
    assert.match(sessionId, /^[0-9a-f-]{36}$/)
  })
})

describe('hello with the launch that hands dsh its stream explicitly', { skip: skipIntegration }, () => {
  it('works as well: the plugin gives the stream the acp row asks for, and a session runs', async () => {
    const model = await ScriptedModel.start(() => ({ text: 'ok' }))
    const dsh = await Dsh.launch({ modelUrl: model.url, recipe: 'stream' })
    try {
      const hello = await dsh.handshake()
      assert.deepEqual(hello.extras.token_stream ?? hello.extras.streaming, { status: 'on' })
      const sessionId = await dsh.newSession()
      assert.equal((await dsh.prompt(sessionId, 'hi')).stopReason, 'end_turn')
      assert.ok(dsh.updates('agent_message_chunk').length > 0)
    } finally {
      await dsh.close()
      await model.close()
    }
  })
})
