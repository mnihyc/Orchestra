/**
 * A scripted model server that speaks OpenAI Chat Completions: streaming text, reasoning and tool calls, with usage
 * that carries cache and reasoning counts. A test gives it one function that answers each request.
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'

/** What the scripted model answers to one request. */
export interface ScriptedReply {
  text?: string
  reasoning?: string
  toolCalls?: Array<{ id?: string; name: string; arguments: unknown }>
  usage?: { prompt: number; completion: number; cached?: number; reasoning?: number }
  finish?: 'stop' | 'tool_calls' | 'length'
  /** Pieces of this many characters make one delta. */
  chunkSize?: number
  /** Wait this long between deltas, in milliseconds. */
  chunkDelayMs?: number
  /** The stream stalls after the first delta until this settles. */
  gate?: Promise<void>
  /** Answer with this HTTP status and no stream. */
  status?: number
}

/** One request the server received. */
export interface RecordedRequest {
  method: string
  path: string
  headers: IncomingMessage['headers']
  body: any
}

/** Answers one request; `index` counts the chat requests of this server from zero. */
export type Respond = (request: RecordedRequest, index: number) => ScriptedReply | Promise<ScriptedReply>

/** The server. */
export class ScriptedModel {
  readonly requests: RecordedRequest[] = []
  readonly chatRequests: RecordedRequest[] = []
  private readonly server: Server
  private readonly respond: Respond
  private readonly sockets = new Set<import('node:net').Socket>()

  private constructor(server: Server, respond: Respond) {
    this.server = server
    this.respond = respond
    server.on('connection', (socket) => {
      this.sockets.add(socket)
      socket.once('close', () => this.sockets.delete(socket))
    })
  }

  /** Starts on a free port of the loopback interface. */
  static async start(respond: Respond): Promise<ScriptedModel> {
    const server = createServer()
    const model = new ScriptedModel(server, respond)
    server.on('request', (req, res) => void model.handle(req, res))
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject)
      server.listen(Number(process.env['ORCH_SCRIPTED_MODEL_PORT'] ?? 0), '127.0.0.1', resolve)
    })
    return model
  }

  /** The base URL a provider is configured with. */
  get url(): string {
    const address = this.server.address() as AddressInfo
    return `http://127.0.0.1:${address.port}/v1`
  }

  /** Stops the server and drops its connections. */
  async close(): Promise<void> {
    for (const socket of this.sockets) socket.destroy()
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }

  private async handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const chunks: Buffer[] = []
    for await (const chunk of req) chunks.push(chunk as Buffer)
    const text = Buffer.concat(chunks).toString('utf8')
    let body: any = undefined
    if (text.length > 0) {
      try {
        body = JSON.parse(text)
      } catch {
        body = text
      }
    }
    const path = (req.url ?? '').split('?')[0] ?? ''
    const recorded: RecordedRequest = { method: req.method ?? 'GET', path, headers: req.headers, body }
    this.requests.push(recorded)
    if (req.method === 'GET' && path.endsWith('/models')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ object: 'list', data: [{ id: 'scripted-model', object: 'model' }] }))
      return
    }
    if (req.method === 'POST' && path.endsWith('/chat/completions')) {
      const index = this.chatRequests.length
      this.chatRequests.push(recorded)
      let reply: ScriptedReply
      try {
        reply = await this.respond(recorded, index)
      } catch (error) {
        res.writeHead(500, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ error: { message: String(error) } }))
        return
      }
      await this.stream(req, res, reply)
      return
    }
    res.writeHead(404, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ error: { message: `no route for ${req.method} ${path}` } }))
  }

  private async stream(req: IncomingMessage, res: ServerResponse, reply: ScriptedReply): Promise<void> {
    if (reply.status !== undefined && reply.status >= 400) {
      res.writeHead(reply.status, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ error: { message: `scripted failure ${reply.status}` } }))
      return
    }
    let gone = false
    req.once('close', () => {
      gone = true
    })
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
    const id = `chatcmpl-${Math.random().toString(16).slice(2, 10)}`
    const send = (delta: Record<string, unknown>, finish: string | null = null): void => {
      if (gone) return
      res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', created: 0, model: 'scripted-model', choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`)
    }
    const size = reply.chunkSize ?? 8
    const pause = async (): Promise<void> => {
      if ((reply.chunkDelayMs ?? 0) > 0) await new Promise((resolve) => setTimeout(resolve, reply.chunkDelayMs))
    }
    send({ role: 'assistant', content: '' })
    let first = true
    const pieces = (value: string): string[] => {
      const out: string[] = []
      for (let at = 0; at < value.length; at += size) out.push(value.slice(at, at + size))
      return out
    }
    for (const piece of pieces(reply.reasoning ?? '')) {
      send({ reasoning_content: piece })
      await pause()
    }
    for (const piece of pieces(reply.text ?? '')) {
      send({ content: piece })
      if (first && reply.gate !== undefined) await reply.gate
      first = false
      await pause()
    }
    if (first && reply.gate !== undefined) await reply.gate
    const calls = reply.toolCalls ?? []
    calls.forEach((call, index) => {
      const args = typeof call.arguments === 'string' ? call.arguments : JSON.stringify(call.arguments)
      send({ tool_calls: [{ index, id: call.id ?? `call_${index}_${id}`, type: 'function', function: { name: call.name, arguments: '' } }] })
      for (const piece of pieces(args)) send({ tool_calls: [{ index, function: { arguments: piece } }] })
    })
    send({}, reply.finish ?? (calls.length > 0 ? 'tool_calls' : 'stop'))
    if (reply.usage !== undefined && !gone) {
      const u = reply.usage
      res.write(
        `data: ${JSON.stringify({
          id,
          object: 'chat.completion.chunk',
          created: 0,
          model: 'scripted-model',
          choices: [],
          usage: {
            prompt_tokens: u.prompt,
            completion_tokens: u.completion,
            total_tokens: u.prompt + u.completion,
            prompt_tokens_details: { cached_tokens: u.cached ?? 0 },
            completion_tokens_details: { reasoning_tokens: u.reasoning ?? 0 },
          },
        })}\n\n`,
      )
    }
    if (!gone) res.write('data: [DONE]\n\n')
    res.end()
  }
}
