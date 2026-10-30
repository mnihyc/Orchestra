/**
 * Skills of the session.
 *
 * dsh reads skills from `skills/<name>/SKILL.md` under its home, among other places, and its `skill` tool loads them.
 * Orch lays out each session's skills in a directory it names in `_meta.orch.skills_dir`. Where that directory is one
 * dsh reads already (its own home), nothing more is done; where it is not, the plugin registers it as one more source
 * of skills, with the folder layout and front matter dsh's own provider reads, so that the catalogue shown to the
 * model and the `skill` tool cover it. The extra also answers `_orch/skills/refresh` with the names a session sees
 * now, which is how Orch confirms that a skill it just wrote is visible.
 */
import { readdir, readFile, realpath } from 'node:fs/promises'
import path from 'node:path'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import { Method, type SkillsRefreshResult } from '../protocol.ts'
import { paramsOf, requireOn, type Agent } from './shared.ts'

interface SkillRegistry {
  list(options?: { scope?: unknown; cwd?: string; signal?: AbortSignal }): Promise<Array<{ name: string; description?: string }>>
  registerProvider?(create: (control: { signal: AbortSignal; invalidate: () => void }) => unknown): () => void
}

interface ParsedSkill {
  name: string
  description: string
  whenToUse?: string
  modelInvocable: boolean
  userInvocable: boolean
  body: string
}

/** Reads `---` front matter: single-line values and the folded and literal block forms. */
export function parseSkill(text: string): ParsedSkill | undefined {
  const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(text)
  if (match === null) return undefined
  const values = new Map<string, string>()
  const lines = (match[1] ?? '').split(/\r?\n/)
  for (let at = 0; at < lines.length; at += 1) {
    const found = /^([A-Za-z][\w-]*):\s*(.*)$/.exec(lines[at] ?? '')
    if (found === null) continue
    const key = found[1] ?? ''
    let value = (found[2] ?? '').trim()
    if (/^[>|][+-]?$/.test(value)) {
      const folded = value.startsWith('>')
      const block: string[] = []
      while (at + 1 < lines.length && /^\s+\S|^\s*$/.test(lines[at + 1] ?? '')) {
        at += 1
        block.push((lines[at] ?? '').trim())
      }
      value = (folded ? block.join(' ') : block.join('\n')).trim()
    } else if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      value = value.slice(1, -1)
    }
    values.set(key, value)
  }
  const name = values.get('name')
  const description = values.get('description')
  if (name === undefined || description === undefined || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) return undefined
  const skill: ParsedSkill = {
    name,
    description,
    modelInvocable: values.get('disable-model-invocation') !== 'true',
    userInvocable: values.get('user-invocable') !== 'false',
    body: (match[2] ?? '').trim(),
  }
  const whenToUse = values.get('when_to_use') ?? values.get('when-to-use')
  if (whenToUse !== undefined) skill.whenToUse = whenToUse
  return skill
}

async function realOrSame(target: string): Promise<string> {
  try {
    return await realpath(target)
  } catch {
    return path.resolve(target)
  }
}

/** The rank of the plugin's source among dsh's own: with the custom directories. */
const CUSTOM_RANK = 300

export const skills: Extra = {
  id: 'skills',
  async start(host: ExtraHost) {
    const registry = await host.waitService<SkillRegistry>('skills')
    if (registry === undefined) throw new ExtraOff('host_api_missing', 'the skills registry is not loaded')
    if (typeof registry.list !== 'function') throw new ExtraOff('host_api_changed', 'the skills registry has no list')
    const directories = new Set<string>()
    let invalidate: (() => void) | undefined
    if (typeof registry.registerProvider === 'function') {
      registry.registerProvider((control) => {
        invalidate = control.invalidate
        return {
          name: 'orch-skills',
          async list() {
            const found: unknown[] = []
            for (const dir of directories) {
              let entries: string[] = []
              try {
                entries = await readdir(dir)
              } catch {
                continue
              }
              for (const entry of entries.sort()) {
                const file = path.join(dir, entry, 'SKILL.md')
                let parsed: ParsedSkill | undefined
                try {
                  parsed = parseSkill(await readFile(file, 'utf8'))
                } catch {
                  continue
                }
                if (parsed === undefined) continue
                found.push({
                  name: parsed.name,
                  description: parsed.description,
                  ...(parsed.whenToUse === undefined ? {} : { whenToUse: parsed.whenToUse }),
                  invocation: { modelInvocable: parsed.modelInvocable, userInvocable: parsed.userInvocable },
                  source: 'custom',
                  provider: 'orch-skills',
                  path: file,
                  resourceBase: { kind: 'directory', path: path.dirname(file) },
                  rank: CUSTOM_RANK,
                  locator: file,
                })
              }
            }
            return found
          },
          async get(candidate: { locator: string; name: string }) {
            const parsed = parseSkill(await readFile(candidate.locator, 'utf8'))
            if (parsed === undefined || parsed.name !== candidate.name) return undefined
            return { ...candidate, content: parsed.body }
          },
        }
      })
      const learn = async (sessionId: string): Promise<void> => {
        const dir = host.book.metaOf(sessionId)?.skills_dir
        if (typeof dir !== 'string' || dir === '') return
        const own = host.service<(...parts: string[]) => string>('dshHomePath')
        const resolved = await realOrSame(dir)
        if (typeof own === 'function' && resolved === (await realOrSame(path.join(own(), 'skills')))) return
        if (directories.has(resolved)) return
        directories.add(resolved)
        invalidate?.()
      }
      const note = (sessionId: string): void => void learn(sessionId).catch((error) => host.failRuntime('skills', error))
      host.book.onOpened(({ sessionId }) => note(sessionId))
      host.book.onPrompt((sessionId) => note(sessionId))
    }

    host.channel.handle(Method.skillsRefresh, async (params): Promise<SkillsRefreshResult> => {
      requireOn(host, 'skills')
      const input = paramsOf(params)
      const agents = host.service<{ get(id: string): Agent | undefined }>('agents')
      const agent = typeof input['sessionId'] === 'string' ? agents?.get(input['sessionId']) : undefined
      const options: { scope?: unknown; cwd?: string } = {}
      if (agent !== undefined) {
        options.scope = agent
        const cwd = agent.session.header.cwd
        if (cwd !== undefined) options.cwd = cwd
      }
      const found = await registry.list(options)
      return { skills: found.map((s) => (s.description === undefined ? { name: s.name } : { name: s.name, description: s.description })) }
    })
  },
}
