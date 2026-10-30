/**
 * History carry: Orch's neutral history written into a new dsh session, and a dsh session's own record written out.
 *
 * A thread that is switched to dsh brings its history with it. Orch writes the history as a file of neutral items; the
 * new session is made from it in dsh's own format (version 4), so that dsh resumes it as it resumes any session of
 * its own, with the conversation in its log and not as a pasted notice. Export copies the stored record of a session
 * so that another dsh of the same format can take it over.
 */
import { randomUUID } from 'node:crypto'
import { copyFile, readdir, readFile, stat } from 'node:fs/promises'
import path from 'node:path'
import { RpcFault } from '../channel.ts'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import {
  ErrorCode,
  Method,
  NATIVE_FORMAT,
  NEUTRAL_FORMAT,
  type HistoryExportResult,
  type HistoryImportResult,
} from '../protocol.ts'
import { neutralToSeed } from '../neutral.ts'
import { defaultRoute, storeSession } from './records.ts'
import { liveAgent, paramsOf, requireOn, stringParam } from './shared.ts'

/** The stored file of a session: `<sessions>/--<cwd>--/<id>/session.v4.jsonl[.zstd]`. */
async function storedFile(sessionsRoot: string, sessionId: string): Promise<string | undefined> {
  for (const bucket of await readdir(sessionsRoot)) {
    const dir = path.join(sessionsRoot, bucket, sessionId)
    try {
      for (const name of await readdir(dir)) if (/^session\.v4\.jsonl(\.zstd)?$/.test(name)) return path.join(dir, name)
    } catch {
      // This bucket does not hold the session.
    }
  }
  return undefined
}

export const history: Extra = {
  id: 'native_carry',
  async start(host: ExtraHost) {
    if ((await host.waitService('agents')) === undefined) throw new ExtraOff('host_api_missing', 'the agent registry is not loaded')
    if ((await host.waitService('sessions')) === undefined) throw new ExtraOff('host_api_missing', 'the session store is not loaded')

    host.channel.handle(Method.historyImport, async (params): Promise<HistoryImportResult> => {
      requireOn(host, 'native_carry')
      const input = paramsOf(params)
      if (input['format'] !== NEUTRAL_FORMAT) throw new RpcFault(ErrorCode.invalidParams, `format must be ${NEUTRAL_FORMAT}`)
      const file = stringParam(input, 'path')
      const cwd = stringParam(input, 'cwd')
      if (!path.isAbsolute(cwd)) throw new RpcFault(ErrorCode.invalidParams, 'cwd must be an absolute path')
      const through = input['throughTurn']
      if (through !== undefined && through !== null && (typeof through !== 'number' || !Number.isInteger(through) || through < 0)) {
        throw new RpcFault(ErrorCode.invalidParams, 'throughTurn must be a whole number')
      }
      let text: string
      try {
        text = await readFile(file, 'utf8')
      } catch (error) {
        throw new RpcFault(ErrorCode.invalidParams, `the history file cannot be read: ${(error as Error).message}`)
      }
      const named = typeof input['provider'] === 'string' && typeof input['model'] === 'string' ? { provider: input['provider'], model: input['model'] } : undefined
      const route = named ?? defaultRoute(host)
      if (route === undefined) throw new RpcFault(ErrorCode.invalidParams, 'provider and model must be given: no session is open to take them from')
      const sessionId = randomUUID()
      let converted: ReturnType<typeof neutralToSeed>
      try {
        converted = neutralToSeed(text, {
          ...route,
          startTime: Date.now(),
          newId: randomUUID,
          ...(typeof through === 'number' ? { throughTurn: through } : {}),
        })
      } catch (error) {
        throw new RpcFault(ErrorCode.invalidParams, (error as Error).message)
      }
      await storeSession(host, { sessionId, cwd, seed: converted.events, ...route })
      return { sessionId, report: converted.report }
    })

    host.channel.handle(Method.historyExport, async (params): Promise<HistoryExportResult> => {
      requireOn(host, 'native_carry')
      const input = paramsOf(params)
      if (input['format'] !== NATIVE_FORMAT) throw new RpcFault(ErrorCode.invalidParams, `format must be ${NATIVE_FORMAT}`)
      const agent = liveAgent(host, input['sessionId'])
      const target = stringParam(input, 'path')
      const home = host.service<(...parts: string[]) => string>('dshHomePath')
      if (typeof home !== 'function') throw new RpcFault(ErrorCode.notNow, 'the home of dsh is not known to this process')
      await host.service<{ flush(session: unknown): Promise<boolean> }>('sessions')?.flush(agent.session)
      const source = await storedFile(home('sessions'), agent.id)
      if (source === undefined) throw new RpcFault(ErrorCode.notNow, 'the session has no stored record yet')
      await copyFile(source, target)
      return { format: NATIVE_FORMAT, path: target, size: (await stat(target)).size }
    })
  },
}
