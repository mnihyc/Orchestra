/** Small readers of the requests a scripted model receives. */
import type { RecordedRequest } from './scripted-model.ts'

/** The text of the last message of a role in a request, or ''. */
export function lastText(request: RecordedRequest, role: 'user' | 'tool' | 'assistant'): string {
  const messages: any[] = request.body.messages
  const last = [...messages].reverse().find((m) => m.role === role)
  if (last === undefined) return ''
  if (typeof last.content === 'string') return last.content
  return JSON.stringify(last.content)
}

/** The names of the tools offered in a request. */
export function toolNames(request: RecordedRequest): string[] {
  return (request.body.tools ?? []).map((t: any) => t.function.name)
}

/** A tool call that needs no approval beyond the profile's own and does nothing. */
export const harmless = { name: 'bash', arguments: { command: 'true', description: 'does nothing' } }
