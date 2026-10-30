/**
 * A minimal MCP server over Streamable HTTP (JSON answers only) with one tool, `echo`, behind a bearer token: what Orch's
 * own tools look like to a program that is handed them by address.
 */
import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'

/** The server. */
export class MiniMcp {
  /** The `tools/call` requests it answered. */
  readonly calls: Array<{ name: string; arguments: any }> = []
  /** The `authorization` header of every request. */
  readonly authorizations: Array<string | undefined> = []
  private readonly server: Server

  private constructor(server: Server) {
    this.server = server
  }

  /** Starts on a free port; only `token` is let in. */
  static async start(token: string): Promise<MiniMcp> {
    const server = createServer()
    const mcp = new MiniMcp(server)
    server.on('request', (req, res) => {
      mcp.authorizations.push(req.headers.authorization)
      const chunks: Buffer[] = []
      req.on('data', (chunk: Buffer) => chunks.push(chunk))
      req.on('end', () => {
        if (req.headers.authorization !== `Bearer ${token}`) return void res.writeHead(401).end('no')
        if (req.method !== 'POST') return void res.writeHead(405).end()
        const message = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (message.id === undefined) return void res.writeHead(202).end()
        const reply = (result: unknown): void => void res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }))
        switch (message.method) {
          case 'initialize':
            return reply({ protocolVersion: message.params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: 'orch-mini', version: '1' } })
          case 'tools/list':
            return reply({ tools: [{ name: 'echo', description: 'Echoes the text.', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] } }] })
          case 'tools/call':
            mcp.calls.push({ name: message.params.name, arguments: message.params.arguments })
            return reply({ content: [{ type: 'text', text: `echo:${message.params.arguments.text}` }] })
          default:
            return reply({})
        }
      })
    })
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject)
      server.listen(Number(process.env['ORCH_MINI_MCP_PORT'] ?? 0), '127.0.0.1', resolve)
    })
    return mcp
  }

  /** Where it answers. */
  get url(): string {
    return `http://127.0.0.1:${(this.server.address() as AddressInfo).port}/mcp`
  }

  /** Stops it. */
  async close(): Promise<void> {
    this.server.closeAllConnections()
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }
}
