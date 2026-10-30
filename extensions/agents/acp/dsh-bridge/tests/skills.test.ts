/** Skills Orch keeps under dsh's home are served by dsh, and a refresh reports what a session sees. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { Dsh, skipIntegration } from './support/dsh.ts'
import { lastText } from './support/model-helpers.ts'
import { ScriptedModel, type ScriptedReply } from './support/scripted-model.ts'

function writeSkill(home: string, name: string, description: string, body: string): void {
  const dir = path.join(home, 'skills', name)
  mkdirSync(dir, { recursive: true })
  writeFileSync(path.join(dir, 'SKILL.md'), `---\nname: ${name}\ndescription: ${description}\n---\n\n${body}\n`)
}

describe('skills', { skip: skipIntegration }, () => {
  let model: ScriptedModel
  let dsh: Dsh
  let sessionId: string
  let home: string
  let plan: ScriptedReply[] = []
  before(async () => {
    home = mkdtempSync(path.join(tmpdir(), 'orch-dsh-skills-'))
    writeSkill(home, 'release-notes', 'Writes release notes in the house style', 'Always start with the date.')
    model = await ScriptedModel.start((_request, index) => plan[index] ?? { text: 'done' })
    dsh = await Dsh.launch({ modelUrl: model.url, home })
    const hello = await dsh.handshake()
    assert.deepEqual(hello.extras.skills, { status: 'on' })
    sessionId = await dsh.newSession()
  })
  after(async () => {
    await dsh.close()
    await model.close()
  })

  it('lists the skills of the session and lets the model load one from the home', async () => {
    const listed = await dsh.request('_orch/skills/refresh', { sessionId })
    assert.deepEqual(listed.skills.map((s: any) => s.name), ['release-notes'])
    plan = [{ toolCalls: [{ name: 'skill', arguments: { name: 'release-notes' } }] }, { text: 'ok' }]
    await dsh.request('session/prompt', { sessionId, prompt: [{ type: 'text', text: 'use the skill' }] })
    assert.match(lastText(model.chatRequests[1]!, 'tool'), /Always start with the date/)
    assert.match(JSON.stringify(model.chatRequests[0]!.body.messages), /release-notes/, 'the catalogue names the skill to the model')
  })

  it('shows a skill written after the session started once Orch asks again', async () => {
    writeSkill(home, 'code-style', 'The code style of the repository', 'Use tabs.')
    let names: string[] = []
    for (let attempt = 0; attempt < 40 && !names.includes('code-style'); attempt += 1) {
      names = ((await dsh.request('_orch/skills/refresh', { sessionId })).skills as any[]).map((s) => s.name)
      if (!names.includes('code-style')) await new Promise((resolve) => setTimeout(resolve, 250))
    }
    assert.deepEqual(names, ['code-style', 'release-notes'])
  })
})
