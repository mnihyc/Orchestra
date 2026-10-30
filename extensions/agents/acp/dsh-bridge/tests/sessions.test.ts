/** Fork, history carry and export against a real dsh and a scripted model. */
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { zstdDecompressSync } from 'node:zlib'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

const everything = (request: { body: any }): string => JSON.stringify(request.body.messages)

describe('fork', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  const answers = ['first answer', 'second answer', 'answer after the fork']
  before(async () => {
    model = await ScriptedModel.start((_request, index): ScriptedReply => ({ text: answers[index] ?? 'more' }))
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.fork, { status: 'on' })
    sessionId = await dsh.newSession()
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'question one' }] })
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'question two' }] })
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('copies a session through a turn into a new session that resumes with only that history', async () => {
    const copy = await dsh.request('_orch/session/fork', { sessionId, throughTurn: 1 })
    assert.notEqual(copy.sessionId, sessionId)
    assert.equal(copy.turns, 1)
    await dsh.request('session/resume', { sessionId: copy.sessionId, cwd: dsh.cwd, mcpServers: [] })
    await dsh.request('session/prompt', { sessionId: copy.sessionId, prompt: [{ type: 'text', text: 'question three' }] })
    const seen = everything(model.chatRequests[2]!)
    assert.match(seen, /question one/)
    assert.match(seen, /first answer/)
    assert.doesNotMatch(seen, /question two/)
    assert.doesNotMatch(seen, /second answer/)
  })

  it('leaves the source as it was, and copies through the last finished turn by default', async () => {
    const copy = await dsh.request('_orch/session/fork', { sessionId })
    assert.equal(copy.turns, 2)
    await assert.rejects(dsh.request('_orch/session/fork', { sessionId, throughTurn: 9 }), (e: Error & { code?: number }) => e.code === -32004)
  })
})

describe('history carry', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let dir: string
  before(async () => {
    dir = mkdtempSync(path.join(tmpdir(), 'orch-carry-'))
    model = await ScriptedModel.start(() => ({ text: 'carried on' }))
    dsh = await Dsh.launch({ modelUrl: model.url })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.native_carry, { status: 'on' })
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  const item = (kind: string, turn: string, payload: unknown, extra: object = {}) => JSON.stringify({ kind, status: 'completed', stream: 'main', turn_id: turn, payload, ...extra })
  const history = [
    item('user_message', 't1', { parts: [{ type: 'text', text: 'Rename the helper to build_index' }] }),
    item('agent_message', 't1', { text: 'I will search for it first.' }),
    item('tool_call', 't1', { title: 'Ran rg helper', tool: { kind: 'command', cwd: '/work', exit_code: 0, output: { inline: 'src/a.rs:3: fn helper()' } } }),
    item('tool_call', 't1', { title: 'Edited src/a.rs', tool: { kind: 'file_change', changes: [{ diff: '-fn helper()\n+fn build_index()' }] } }),
    item('reasoning', 't1', { text: 'hidden thoughts' }),
    item('agent_message', 't1', { text: 'Renamed in one file.' }),
    item('plan', 't2', { steps: [{ status: 'completed', text: 'rename' }, { status: 'in_progress', text: 'run tests' }] }),
    item('user_message', 't2', { parts: [{ type: 'text', text: 'Now run the tests' }] }),
    item('agent_message', 't2', { text: 'Tests pass.' }, { stream: 'subagent:1' }),
    item('agent_message', 't2', { text: 'All tests pass.' }),
  ].join('\n') + '\n'

  for (const throughTurn of [undefined, null]) it(`writes the whole history with ${throughTurn === null ? 'a null' : 'an absent'} turn limit into a new session dsh resumes`, async () => {
    const file = path.join(dir, 'history.jsonl')
    writeFileSync(file, history)
    const result = await dsh.request('_orch/history/import', { path: file, format: 'orch-neutral-jsonl-v1', cwd: dsh.cwd, provider: 'scripted', model: 'scripted-model', ...(throughTurn === null ? { throughTurn } : {}) })
    assert.match(result.sessionId, /^[0-9a-f-]{36}$/)
    assert.equal(result.report.fidelity, 'structured')
    assert.deepEqual(result.report.byKind.user_message, { kept: 2, converted: 0, dropped: 0 })
    assert.deepEqual(result.report.byKind.tool_call, { kept: 0, converted: 2, dropped: 0 })
    assert.equal(result.report.byKind.reasoning.dropped, 1)
    assert.equal(result.report.byKind.agent_message.dropped, 1)
    assert.ok(result.report.dropped.some((d: any) => d.kind === 'agent_message' && /sub-agent/.test(d.reason)))
    await dsh.request('session/resume', { sessionId: result.sessionId, cwd: dsh.cwd, mcpServers: [] })
    const before = model.chatRequests.length
    await dsh.request('session/prompt', { sessionId: result.sessionId, prompt: [{ type: 'text', text: 'Anything left?' }] })
    const messages: any[] = model.chatRequests[before]!.body.messages.filter((m: any) => m.role !== 'system')
    const text = (m: any): string => (typeof m.content === 'string' ? m.content : JSON.stringify(m.content))
    const roles = messages.map((m) => m.role)
    assert.deepEqual(roles.slice(0, 4), ['user', 'assistant', 'user', 'assistant'])
    assert.match(text(messages[0]), /Rename the helper to build_index/)
    assert.match(text(messages[1]), /I will search for it first\.[\s\S]*Steps of this turn:[\s\S]*Ran rg helper · exit 0[\s\S]*fn helper\(\)[\s\S]*Edited src\/a\.rs[\s\S]*\+fn build_index\(\)[\s\S]*Renamed in one file\./)
    assert.doesNotMatch(text(messages[1]), /hidden thoughts/)
    assert.match(text(messages[3]), /Plan:[\s\S]*\[x\] rename[\s\S]*\[~\] run tests[\s\S]*All tests pass\./)
    assert.doesNotMatch(JSON.stringify(messages), /Tests pass\.(?! )/)
    assert.match(text(messages[4]!), /Anything left\?/)
  })

  it('carries only the first turns when asked, and refuses a file that is not history', async () => {
    const file = path.join(dir, 'again.jsonl')
    writeFileSync(file, history)
    const result = await dsh.request('_orch/history/import', { path: file, format: 'orch-neutral-jsonl-v1', cwd: dsh.cwd, provider: 'scripted', model: 'scripted-model', throughTurn: 1 })
    await dsh.request('session/resume', { sessionId: result.sessionId, cwd: dsh.cwd, mcpServers: [] })
    const before = model.chatRequests.length
    await dsh.request('session/prompt', { sessionId: result.sessionId, prompt: [{ type: 'text', text: 'next' }] })
    assert.match(everything(model.chatRequests[before]!), /Rename the helper to build_index/)
    assert.doesNotMatch(everything(model.chatRequests[before]!), /Now run the tests/)
    const bad = path.join(dir, 'bad.jsonl')
    writeFileSync(bad, 'not json\n')
    await assert.rejects(dsh.request('_orch/history/import', { path: bad, format: 'orch-neutral-jsonl-v1', cwd: dsh.cwd, provider: 'scripted', model: 'scripted-model' }), (e: Error & { code?: number }) => e.code === -32602)
  })

  it('writes the stored record of a session out in dsh\'s own format', async () => {
    const sessionId = await dsh.newSession()
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'record me' }] })
    const target = path.join(dir, 'record.zst')
    const result = await dsh.request('_orch/history/export', { sessionId, format: 'dsh-session-v4', path: target })
    assert.equal(result.format, 'dsh-session-v4')
    assert.equal(existsSync(target), true)
    assert.ok(result.size > 100)
    const first = zstdDecompressSync(readFileSync(target)).toString('utf8').split('\n')[0]!
    assert.equal(JSON.parse(first).version, 4)
    assert.equal(JSON.parse(first).id, sessionId)
  })
})
