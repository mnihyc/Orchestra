/** The line channel: what it keeps for the plugin, what it passes on, and how it answers. */
import assert from 'node:assert/strict'
import { PassThrough } from 'node:stream'
import { describe, it } from 'node:test'
import { Channel, RpcFault } from '../src/channel.ts'

function open(): { channel: Channel; input: PassThrough; written: string[]; logs: string[]; forServer: () => string[] } {
  const input = new PassThrough()
  const written: string[] = []
  const logs: string[] = []
  const channel = new Channel({
    input,
    output: { write: (line) => (written.push(line), true), once: () => undefined },
    log: (line) => logs.push(line),
  })
  const read: string[] = []
  channel.serverInput.setEncoding('utf8')
  channel.serverInput.on('data', (chunk: string) => read.push(...chunk.split('\n').filter((l) => l.length > 0)))
  return { channel, input, written, logs, forServer: () => read }
}

const line = (value: unknown): string => `${JSON.stringify(value)}\n`
const sent = (written: string[]): any[] => written.map((l) => JSON.parse(l))
const settle = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 20))

describe('channel', () => {
  it('passes protocol lines on to the server and keeps the plugin\'s own methods for itself', async () => {
    const { channel, input, written, forServer } = open()
    channel.handle('_orch/ping', () => ({ pong: true }))
    input.write(line({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {} }))
    input.write(line({ jsonrpc: '2.0', id: 2, method: '_orch/ping', params: {} }))
    input.write(line({ jsonrpc: '2.0', method: 'session/cancel', params: { sessionId: 's' } }))
    await settle()
    assert.deepEqual(forServer().map((l) => JSON.parse(l).method), ['initialize', 'session/cancel'])
    assert.deepEqual(sent(written), [{ jsonrpc: '2.0', id: 2, result: { pong: true } }])
  })

  it('answers a standard method itself when asked to, and never passes it on', async () => {
    const { channel, input, written, forServer } = open()
    channel.intercept('session/set_mode', (params) => ({ got: params }))
    input.write(line({ jsonrpc: '2.0', id: 3, method: 'session/set_mode', params: { modeId: 'plan' } }))
    await settle()
    assert.deepEqual(forServer(), [])
    assert.deepEqual(sent(written), [{ jsonrpc: '2.0', id: 3, result: { got: { modeId: 'plan' } } }])
  })

  it('answers an unknown method of its own as method not found, and a failing handler as an error', async () => {
    const { channel, input, written } = open()
    channel.handle('_orch/fails', () => {
      throw new RpcFault(-32004, 'not now')
    })
    channel.handle('_orch/breaks', () => {
      throw new Error('boom')
    })
    input.write(line({ jsonrpc: '2.0', id: 1, method: '_orch/unknown' }))
    input.write(line({ jsonrpc: '2.0', id: 2, method: '_orch/fails' }))
    input.write(line({ jsonrpc: '2.0', id: 3, method: '_orch/breaks' }))
    await settle()
    const answers = sent(written)
    assert.equal(answers.find((a) => a.id === 1).error.code, -32601)
    assert.deepEqual(answers.find((a) => a.id === 2).error, { code: -32004, message: 'not now' })
    assert.equal(answers.find((a) => a.id === 3).error.code, -32603)
  })

  it('sends its own request and settles it with the answer the peer gives, apart from the server\'s ids', async () => {
    const { channel, input, written, forServer } = open()
    const answer = channel.request<{ ok: boolean }>('_orch/question/ask', { a: 1 })
    const asked = sent(written)[0]
    assert.match(asked.id, /^orch-/)
    input.write(line({ jsonrpc: '2.0', id: 7, result: { server: true } }))
    input.write(line({ jsonrpc: '2.0', id: asked.id, result: { ok: true } }))
    assert.deepEqual(await answer, { ok: true })
    assert.deepEqual(forServer().map((l) => JSON.parse(l).result), [{ server: true }], 'an answer for the server goes to the server')
  })

  it('tells the peer when a request is withdrawn, and rejects pending requests when the input ends', async () => {
    const { channel, input, written } = open()
    const controller = new AbortController()
    const withdrawn = channel.request('elicitation/create', {}, { signal: controller.signal })
    const id = sent(written)[0].id
    controller.abort()
    await assert.rejects(withdrawn, (e: RpcFault) => e.code === -32004)
    assert.deepEqual(sent(written)[1], { jsonrpc: '2.0', method: '$/cancel_request', params: { requestId: id } })
    const pending = channel.request('elicitation/create', {})
    input.end()
    await assert.rejects(pending, (e: RpcFault) => e.code === -32004)
    assert.equal(channel.isClosed, true)
    await assert.rejects(channel.request('elicitation/create', {}), (e: RpcFault) => e.code === -32004)
  })

  it('skips a line that is not JSON and says so, without losing the lines after it', async () => {
    const { input, logs, forServer } = open()
    input.write('this is not json\n')
    input.write(line({ jsonrpc: '2.0', id: 1, method: 'initialize' }))
    await settle()
    assert.deepEqual(forServer().map((l) => JSON.parse(l).method), ['initialize'])
    assert.equal(logs.length, 1)
  })

  it('writes what the server writes in order with the plugin\'s notifications, and lets a tap change or drop it', async () => {
    const { channel, written } = open()
    channel.tapOutgoing((m) => ((m as any).method === 'drop' ? null : m))
    channel.tapOutgoing((m) => ((m as any).method === 'tag' ? { ...m, params: { tagged: true } } : m))
    const write = (message: unknown): Promise<void> => new Promise((resolve, reject) => channel.serverOutput.write(line(message), (error) => (error ? reject(error) : resolve())))
    await write({ jsonrpc: '2.0', method: 'one' })
    channel.notify('_orch/two', { n: 2 })
    await write({ jsonrpc: '2.0', method: 'drop' })
    await write({ jsonrpc: '2.0', method: 'tag', params: {} })
    assert.deepEqual(sent(written).map((m) => [m.method, m.params]), [['one', undefined], ['_orch/two', { n: 2 }], ['tag', { tagged: true }]])
  })

  it('sends a stray write of another plugin to the log, not to the wire, and joins a line split over writes', async () => {
    const { channel, written, logs } = open()
    const write = (text: string): Promise<void> => new Promise((resolve, reject) => channel.serverOutput.write(text, (error) => (error ? reject(error) : resolve())))
    await write('hello from a stray console.log\n')
    await write('{"jsonrpc":"2.0",')
    await write('"method":"whole"}\n')
    assert.deepEqual(logs, ['hello from a stray console.log'])
    assert.deepEqual(sent(written).map((m) => m.method), ['whole'])
  })
})
