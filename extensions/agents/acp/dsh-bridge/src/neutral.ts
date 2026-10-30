/**
 * Orch's neutral history, written as the log of a dsh session.
 *
 * The neutral history is the main stream of a conversation as JSON lines of items, oldest first. This module turns it
 * into the events dsh's session log holds (format 4), turn by turn: what the person sent becomes a user message, and
 * what the agent said and did in the turn becomes one assistant message. The steps of a turn (what it ran, changed
 * and read) are written out in words, in the same bounded form Orch's other adapters use when they cannot take the
 * steps as they were, so a model that continues the thread reads the same account whichever program it runs in.
 *
 * The result is a seed for a session; nothing here touches the disk.
 */
import type { HistoryReport } from './protocol.ts'

/** The most characters of one text of a step that is written out. */
export const STEP_TEXT_MAX_CHARS = 2000
/** The most characters of a difference that is written out. */
export const STEP_DIFF_MAX_CHARS = 4000
/** How many matches of a file search are written out. */
export const STEP_MATCHES_SHOWN = 20
/** How many results of a web search are written out. */
export const STEP_RESULTS_SHOWN = 10

/** One event of a session log, as dsh stores it. */
export interface SeedEvent {
  type: string
  seq: number
  time: number
  data: Record<string, unknown>
  surfaceOp?: 'append'
}

/** What the converter needs from its caller. */
export interface SeedOptions {
  /** The route recorded on the assistant messages. */
  provider: string
  model: string
  /** Time of the first event, in epoch milliseconds; later events are one millisecond apart. */
  startTime: number
  /** Makes a message id. */
  newId: () => string
  /** Carry only the first turns. */
  throughTurn?: number
}

type Json = Record<string, any>

interface Turn {
  id: string | null
  items: Json[]
}

/** Tracks what became of each item, for the report. */
class Tally {
  private readonly kinds = new Map<string, { kept: number; converted: number; dropped: number }>()
  private readonly reasons = new Map<string, number>()
  readonly warnings: string[] = []
  private entry(kind: string): { kept: number; converted: number; dropped: number } {
    let found = this.kinds.get(kind)
    if (found === undefined) {
      found = { kept: 0, converted: 0, dropped: 0 }
      this.kinds.set(kind, found)
    }
    return found
  }
  keep(kind: string): void {
    this.entry(kind).kept += 1
  }
  convert(kind: string): void {
    this.entry(kind).converted += 1
  }
  drop(kind: string, reason: string): void {
    this.entry(kind).dropped += 1
    const key = `${kind}\u0000${reason}`
    this.reasons.set(key, (this.reasons.get(key) ?? 0) + 1)
  }
  report(): HistoryReport {
    const byKind: HistoryReport['byKind'] = {}
    let itemsTotal = 0
    for (const [kind, counts] of this.kinds) {
      byKind[kind] = { ...counts }
      itemsTotal += counts.kept + counts.converted + counts.dropped
    }
    const dropped = [...this.reasons].map(([key, count]) => {
      const [kind = '', reason = ''] = key.split('\u0000')
      return { kind, count, reason }
    })
    return { fidelity: 'structured', itemsTotal, byKind, dropped, warnings: this.warnings }
  }
}

function clipStart(text: string, most: number): [string, boolean] {
  const chars = [...text]
  return chars.length <= most ? [text, false] : [chars.slice(0, most).join(''), true]
}

function clipEnd(text: string, most: number): [string, boolean] {
  const chars = [...text]
  return chars.length <= most ? [text, false] : [chars.slice(chars.length - most).join(''), true]
}

function fenced(language: string, body: string): string {
  return `\`\`\`${language}\n${body.replace(/\n+$/, '')}\n\`\`\``
}

function indent(block: string): string {
  return block
    .split('\n')
    .map((line) => `  ${line}`)
    .join('\n')
}

/** How a step ended, in the words of the written form: ` · exit 1`, ` · failed`, ` · still running`. */
function ending(item: Json): string {
  const code = item['payload']?.tool?.exit_code
  if (typeof code === 'number') return ` · exit ${code}`
  if (item['status'] === 'failed') return ' · failed'
  if (item['status'] === 'in_progress') return ' · still running'
  return ''
}

function stepText(item: Json): string | undefined {
  const payload = item['payload']
  if (payload === undefined || payload === null) return undefined
  const title = typeof payload.title === 'string' ? payload.title : 'A step'
  let text = `- ${title}${ending(item)}`
  const tool: Json = payload.tool ?? {}
  const detail: string[] = []
  const leftOut = (what: string): void => {
    detail.push(`(${what} was left out here)`)
  }
  switch (tool.kind) {
    case 'command': {
      if (typeof tool.cwd === 'string' && tool.cwd !== '') detail.push(`Directory: ${tool.cwd}`)
      const output: unknown = tool.output?.inline
      if (typeof output === 'string' && output.trim() !== '') {
        const [shown, cut] = clipEnd(output, STEP_TEXT_MAX_CHARS)
        detail.push(fenced('', shown))
        if (cut) leftOut('the beginning of the output')
      }
      break
    }
    case 'file_change':
      for (const change of Array.isArray(tool.changes) ? tool.changes : []) {
        if (typeof change?.diff === 'string' && change.diff !== '') {
          const [shown, cut] = clipStart(change.diff, STEP_DIFF_MAX_CHARS)
          detail.push(fenced('diff', shown))
          if (cut) leftOut('the rest of the difference')
        }
      }
      break
    case 'file_search':
      detail.push(`Looked for ${JSON.stringify(tool.query ?? '')}${typeof tool.path === 'string' ? ` in ${tool.path}` : ''}`)
      for (const hit of (Array.isArray(tool.matches) ? tool.matches : []).slice(0, STEP_MATCHES_SHOWN)) detail.push(String(hit?.path ?? ''))
      break
    case 'web_search':
      detail.push(`Searched for ${JSON.stringify(tool.query ?? '')}`)
      for (const result of (Array.isArray(tool.results) ? tool.results : []).slice(0, STEP_RESULTS_SHOWN)) {
        detail.push(`${result?.title ?? ''} ${result?.url ?? ''}`)
      }
      break
    case 'mcp':
    case 'orch':
    case 'other':
      for (const [label, member] of [['Arguments', 'arguments'], ['Result', 'result']] as const) {
        const value: unknown = tool[member]
        if (value !== undefined && value !== null) {
          const [shown, cut] = clipStart(JSON.stringify(value), STEP_TEXT_MAX_CHARS)
          detail.push(`${label}: ${shown}`)
          if (cut) leftOut('the rest of it')
        }
      }
      break
    case 'subagent':
      if (typeof tool.prompt === 'string') detail.push(`Asked: ${clipStart(tool.prompt, STEP_TEXT_MAX_CHARS)[0]}`)
      break
    default:
      break
  }
  if (detail.length > 0) text += `\n${indent(detail.join('\n'))}`
  return text
}

function planText(item: Json): string | undefined {
  const steps: unknown = item['payload']?.steps
  if (!Array.isArray(steps) || steps.length === 0) return undefined
  const lines = ['Plan:']
  for (const step of steps) {
    const mark = step?.status === 'completed' ? '[x]' : step?.status === 'in_progress' ? '[~]' : '[ ]'
    lines.push(`- ${mark} ${step?.text ?? ''}`)
  }
  return lines.join('\n')
}

function userText(item: Json): string {
  const parts: unknown = item['payload']?.parts
  if (!Array.isArray(parts)) return ''
  const out: string[] = []
  for (const part of parts) {
    switch (part?.type) {
      case 'text':
        if (typeof part.text === 'string') out.push(part.text)
        break
      case 'image':
        out.push('[image omitted]')
        break
      case 'file':
        out.push(`Attached file: ${part.path ?? ''}`)
        break
      case 'forwarded':
        if (typeof part.text === 'string') out.push(`[Forwarded]\n${part.text}`)
        break
      default:
        break
    }
  }
  return out.join('\n\n')
}

/** Groups the main-stream items into turns, by the turn each item names; the report counts what is left behind. */
function turnsOf(lines: string[], tally: Tally): Turn[] {
  const turns: Turn[] = []
  for (const line of lines) {
    if (line.trim() === '') continue
    let item: Json
    try {
      item = JSON.parse(line) as Json
    } catch {
      throw new Error('a line of the history is not JSON')
    }
    const kind = typeof item['kind'] === 'string' ? item['kind'] : 'item'
    if ((item['stream'] ?? 'main') !== 'main') {
      tally.drop(kind, "it belongs to a sub-agent's own stream")
      continue
    }
    const id = typeof item['turn_id'] === 'string' ? item['turn_id'] : typeof item['turn_ordinal'] === 'number' ? String(item['turn_ordinal']) : null
    const last = turns[turns.length - 1]
    if (last !== undefined && last.id === id) last.items.push(item)
    else turns.push({ id, items: [item] })
  }
  return turns
}

/** One turn's assistant text: what the agent said, the steps in words, the plan, and the final answer last. */
function assistantText(turn: Turn, tally: Tally): { user: string | undefined; assistant: string | undefined } {
  let user: string | undefined
  const said: string[] = []
  const steps: string[] = []
  let plan: string | undefined
  for (const item of turn.items) {
    const kind = typeof item['kind'] === 'string' ? item['kind'] : 'item'
    if (kind === 'user_message') {
      const text = userText(item)
      if (text === '') tally.drop(kind, 'it carried nothing the model can be told')
      else {
        tally.keep(kind)
        user = user === undefined ? text : `${user}\n\n${text}`
      }
    } else if (kind === 'agent_message') {
      const text: unknown = item['payload']?.text
      if (typeof text !== 'string' || text === '') tally.drop(kind, 'it said nothing')
      else {
        tally.keep(kind)
        said.push(text)
      }
    } else if (kind === 'tool_call') {
      const written = stepText(item)
      if (written !== undefined) {
        tally.convert(kind)
        steps.push(written)
      }
    } else if (kind === 'plan') {
      const written = planText(item)
      if (written === undefined) tally.drop(kind, 'it had no steps')
      else {
        tally.convert(kind)
        plan = written
      }
    } else {
      tally.drop(kind, 'the model is not told of it')
    }
  }
  const final = said.pop()
  const parts = [...said]
  const work: string[] = []
  if (steps.length > 0) work.push(`Steps of this turn:\n${steps.join('\n')}`)
  if (plan !== undefined) work.push(plan)
  if (work.length > 0) parts.push(work.join('\n\n'))
  if (final !== undefined) parts.push(final)
  return { user, assistant: parts.length > 0 ? parts.join('\n\n') : undefined }
}

/**
 * Converts a neutral history into the events of a new session.
 * @param text - the history file's text
 * @param options - the route recorded on the messages, ids and times
 * @returns the events, contiguous from seq 0, the number of turns carried, and what became of each item
 */
export function neutralToSeed(text: string, options: SeedOptions): { events: SeedEvent[]; turns: number; report: HistoryReport } {
  const tally = new Tally()
  let turns = turnsOf(text.split('\n'), tally)
  if (options.throughTurn !== undefined) turns = turns.slice(0, options.throughTurn)
  const events: SeedEvent[] = []
  let time = options.startTime
  const push = (type: string, data: Record<string, unknown>, surface = false): SeedEvent => {
    const event: SeedEvent = { type, seq: events.length, time: time++, data }
    if (surface) event.surfaceOp = 'append'
    events.push(event)
    return event
  }
  let carried = 0
  let headerWritten = false
  for (const turn of turns) {
    const { user, assistant } = assistantText(turn, tally)
    if (user === undefined && assistant === undefined) continue
    carried += 1
    push('turn/start', { turn: carried })
    push('step/start', { turn: carried, step: 1 })
    if (user !== undefined) {
      push('user/message', { content: [{ type: 'text', text: user }], source: { kind: 'user' }, role: 'user', id: options.newId() }, true)
    }
    if (assistant !== undefined) {
      if (!headerWritten) {
        push('request/header', { header: { config: { provider: options.provider, model: options.model } }, reason: 'initial', startsSeries: true })
        push('request/context', { provider: options.provider, model: options.model })
        headerWritten = true
      }
      const at = time
      push(
        'assistant/message',
        {
          turn: carried,
          step: 1,
          message: {
            role: 'assistant',
            content: [{ type: 'text', text: assistant }],
            source: { kind: 'model', provider: options.provider, model: options.model },
            id: options.newId(),
          },
          stream: [
            { type: 'chunk', time: at, chunk: { type: 'block-start', index: 0, blockType: 'text' } },
            { type: 'text-chunks', time0: at, index: 0, dt: [], texts: [assistant] },
            { type: 'chunk', time: at, chunk: { type: 'block-end', index: 0, block: { type: 'text', text: assistant } } },
            { type: 'chunk', time: at, chunk: { type: 'finish', reason: { kind: 'stop' } } },
          ],
        },
        true,
      )
    }
    push('step/end', { turn: carried, step: 1 })
    push('turn/end', { turn: carried, reason: { kind: 'completed' } })
  }
  return { events, turns: carried, report: tally.report() }
}
