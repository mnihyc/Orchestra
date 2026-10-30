/** Compaction phases and a manual run against a real dsh and a scripted model. */
import assert from 'node:assert/strict'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { ScriptedModel } from './support/scripted-model.ts'

const long = (word: string): string => `${word} `.repeat(1500)

describe('compaction', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  before(async () => {
    model = await ScriptedModel.start((request) => {
      const text = JSON.stringify(request.body.messages)
      if (/summar/i.test(JSON.stringify(request.body.messages.at(-1)))) return { text: 'SUMMARY of the earlier turns.' }
      return { text: `reply ${text.length}`, usage: { prompt: 10, completion: 5 } }
    })
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.compaction, { status: 'on' })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('reports the phases of a compaction that Orch asked for, in order', async () => {
    for (const word of ['alpha', 'beta', 'gamma', 'delta', 'epsilon']) {
      await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: long(word) }] })
    }
    const result = await dsh.request('_orch/compaction/run', { sessionId }, 120000)
    assert.equal(result.compacted, true)
    const phases = dsh.of('_orch/compaction').map((m) => (m.params as any).phase)
    assert.deepEqual(phases.filter((p) => p !== 'pruned'), ['started', 'summarized', 'completed'])
    const summarized = dsh.of('_orch/compaction').map((m) => m.params as any).find((p) => p.phase === 'summarized')
    assert.ok(summarized.replacedTokens > 0)
    assert.equal(summarized.sessionId, sessionId)
    assert.equal(summarized.turn, null, 'a run between turns has no turn')
  })

  it('says nothing was compacted when there is nothing worth compacting', async () => {
    const fresh = await dsh.newSession()
    const result = await dsh.request('_orch/compaction/run', { sessionId: fresh }, 60000)
    assert.equal(result.compacted, false)
  })
})
