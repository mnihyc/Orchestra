/**
 * Orch's plugin for DeepSeek Harness.
 *
 * dsh's Agent Client Protocol server carries sessions, prompts, tool calls, permissions and cancellation, and leaves
 * out what Orch needs to treat dsh like its other agent programs: live token streaming, steering, questions, an
 * enforced plan mode, the token counts of each model call, compaction phases, per-session skills, fork, history
 * carry, and commands and files that run in an environment of Orch's. This plugin adds each of them as an extra,
 * in the protocol's own messages where it has them and under `_orch/` where it does not, and tells Orch which extras
 * work in `_orch/hello`. An extra that cannot work in a dsh build is off with its reason; the others keep working, and
 * the plain protocol is never touched.
 *
 * It is loaded by a patch file, as a row with an absolute module path:
 *
 *   - insert:
 *       - id: orch-dsh-bridge
 *         name: /absolute/path/orch-dsh-bridge.mjs
 *         config: {}
 *
 * Its module takes over the stdio of the process when it is evaluated (see `takeover.ts`), which happens before any
 * row is applied; `apply` then starts the extras.
 */
import { Bridge } from './bridge.ts'
import { errorText } from './channel.ts'
import { resolveConfig } from './config.ts'
import type { Ctx } from './host.ts'
import { PLUGIN_NAME } from './protocol.ts'
import { EXTRAS } from './extras/index.ts'
import { takeOverStdio } from './takeover.ts'

/** Cordis plugin name. */
export const name = PLUGIN_NAME

const log = (line: string): void => {
  process.stderr.write(`${line}\n`)
}

const channel = takeOverStdio(log)

/**
 * The plugin entry. Dsh calls it once with its Cordis context and the row's config.
 * @param ctx - the Cordis context
 * @param rawConfig - the `config` of the row, or undefined
 */
export function apply(ctx: Ctx, rawConfig?: unknown): void {
  const config = resolveConfig(rawConfig)
  if (channel === undefined) {
    log('orch-dsh-bridge: this process does not serve the protocol on its stdio; the plugin does nothing')
    return
  }
  // A profile that gives dsh's ACP server its stream explicitly (`stream: !!js ctx.orchBridge.stream`, after
  // `inject: [orchBridge]` on the `acp` row) finds it here; a profile that does not never reads it.
  ctx.provide('orchBridge', { stream: channel.asMessageStream() })
  const bridge = new Bridge(ctx, config, EXTRAS, channel, log)
  // The extras start after this function returns, so that the services of dsh that load later are there to be found.
  queueMicrotask(() => {
    void bridge.start().catch((error) => log(`orch-dsh-bridge: start failed: ${errorText(error)}`))
  })
}
