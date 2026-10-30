/** Orch's neutral history written as the events of a dsh session. */
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { neutralToSeed, STEP_TEXT_MAX_CHARS } from '../src/neutral.ts'

let counter = 0
const options = { provider: 'p', model: 'm', startTime: 1000, newId: () => `id-${++counter}` }
const item = (kind: string, turn: string | number, payload: unknown, extra: object = {}): string =>
  JSON.stringify({ kind, status: 'completed', stream: 'main', turn_id: turn, payload, ...extra })

describe('neutral history to session events', () => {
  it('writes each turn as a closed turn with its user message and one assistant message', () => {
    const text = [item('user_message', 'a', { parts: [{ type: 'text', text: 'hi' }] }), item('agent_message', 'a', { text: 'hello' }), item('user_message', 'b', { parts: [{ type: 'text', text: 'again' }] }), item('agent_message', 'b', { text: 'sure' })].join('\n')
    const { events, turns } = neutralToSeed(text, options)
    assert.equal(turns, 2)
    assert.deepEqual(events.map((e) => e.seq), events.map((_, i) => i), 'sequence numbers are contiguous from zero')
    assert.deepEqual(events.filter((e) => e.type.startsWith('turn/')).map((e) => `${e.type}:${(e.data as any).turn}`), ['turn/start:1', 'turn/end:1', 'turn/start:2', 'turn/end:2'])
    const assistants = events.filter((e) => e.type === 'assistant/message')
    assert.equal(assistants.length, 2)
    assert.equal((assistants[0]!.data as any).message.content[0].text, 'hello')
    assert.equal((assistants[0]!.data as any).message.source.provider, 'p')
    assert.equal((assistants[0]!.data as any).usage, undefined, 'a carried message has no token counts, not zero ones')
  })

  it('tells the steps in words before the final answer and leaves out what the model must not be told', () => {
    const text = [
      item('user_message', 'a', { parts: [{ type: 'text', text: 'do it' }, { type: 'image', url: 'x' }] }),
      item('agent_message', 'a', { text: 'working' }),
      item('tool_call', 'a', { title: 'Ran make', tool: { kind: 'command', cwd: '/w', exit_code: 2, output: { inline: 'boom' } } }, { status: 'failed' }),
      item('reasoning', 'a', { text: 'secret' }),
      item('agent_message', 'a', { text: 'it failed' }),
    ].join('\n')
    const { events, report } = neutralToSeed(text, options)
    const user = events.find((e) => e.type === 'user/message')!
    assert.equal((user.data as any).content[0].text, 'do it\n\n[image omitted]')
    const assistant = (events.find((e) => e.type === 'assistant/message')!.data as any).message.content[0].text as string
    assert.match(assistant, /^working\n\nSteps of this turn:\n- Ran make · exit 2\n {2}Directory: \/w\n {2}```\n {2}boom\n {2}```\n\nit failed$/)
    assert.doesNotMatch(assistant, /secret/)
    assert.deepEqual(report.byKind['reasoning'], { kept: 0, converted: 0, dropped: 1 })
    assert.deepEqual(report.byKind['tool_call'], { kept: 0, converted: 1, dropped: 0 })
    assert.equal(report.itemsTotal, 5)
    assert.equal(report.fidelity, 'structured')
  })

  it('leaves a sub-agent stream out with its reason, and carries only the first turns when asked', () => {
    const text = [item('user_message', 'a', { parts: [{ type: 'text', text: 'one' }] }), item('agent_message', 'a', { text: 'x' }, { stream: 'sub' }), item('user_message', 'b', { parts: [{ type: 'text', text: 'two' }] })].join('\n')
    const all = neutralToSeed(text, options)
    assert.equal(all.turns, 2)
    assert.deepEqual(all.report.dropped, [{ kind: 'agent_message', count: 1, reason: "it belongs to a sub-agent's own stream" }])
    assert.equal(neutralToSeed(text, { ...options, throughTurn: 1 }).turns, 1)
  })

  it('bounds the output of a command it writes out, keeping the end of it', () => {
    const output = `${'x'.repeat(STEP_TEXT_MAX_CHARS)}END`
    const text = [item('user_message', 'a', { parts: [{ type: 'text', text: 'run' }] }), item('tool_call', 'a', { title: 'Ran long', tool: { kind: 'command', output: { inline: output } } })].join('\n')
    const assistant = (neutralToSeed(text, options).events.find((e) => e.type === 'assistant/message')!.data as any).message.content[0].text as string
    assert.match(assistant, /END\n {2}```/)
    assert.match(assistant, /the beginning of the output was left out here/)
    assert.ok(!assistant.includes('x'.repeat(STEP_TEXT_MAX_CHARS)), 'the text is cut to its bound, so the whole run of x is not there')
  })

  it('refuses a line that is not JSON', () => {
    assert.throws(() => neutralToSeed('not json', options), /not JSON/)
  })

  it('writes nothing for an empty history', () => {
    const { events, turns, report } = neutralToSeed('\n', options)
    assert.deepEqual([events.length, turns, report.itemsTotal], [0, 0, 0])
  })
})
