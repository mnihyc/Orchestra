/** A tool of Orch handed to the program by address (MCP over HTTP with a bearer token), beside the plugin's own work. */
import assert from 'node:assert/strict'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { lastText } from './support/model-helpers.ts'
import { MiniMcp } from './support/mini-mcp.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

describe('tools handed over by address', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let mcp: MiniMcp
  let plan: ScriptedReply[] = []
  before(async () => {
    mcp = await MiniMcp.start('orch-tool-token')
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url, bridge: { approvals: { mode: 'bypass' } } })
    await dsh.handshake()
  })
  after(async () => {
    await dsh.close()
    await model.close()
    await mcp.close()
  })

  it('offers the tool of the server named in session/new, calls it with the token and gives the model its answer', async () => {
    const sessionId = await dsh.newSession(undefined, [{ type: 'http', name: 'orch', url: mcp.url, headers: [{ name: 'Authorization', value: 'Bearer orch-tool-token' }] }])
    plan = [{ text: 'looking' }]
    await dsh.prompt(sessionId, 'hello')
    const names: string[] = model.chatRequests[0]!.body.tools.map((t: any) => t.function.name)
    const echo = names.find((name) => name.endsWith('echo'))
    assert.ok(echo !== undefined, `the tool is offered among ${names.join(', ')}`)
    const base = model.chatRequests.length
    plan = [...Array(base).fill({ text: 'skip' }), { toolCalls: [{ id: 'c-mcp', name: echo, arguments: { text: 'ping' } }] }, { text: 'ok' }]
    await dsh.prompt(sessionId, 'use it')
    assert.deepEqual(mcp.calls, [{ name: 'echo', arguments: { text: 'ping' } }])
    assert.match(lastText(model.chatRequests[base + 1]!, 'tool'), /echo:ping/)
    assert.ok(mcp.authorizations.every((value) => value === 'Bearer orch-tool-token'), 'every request carried the token')
  })
})
