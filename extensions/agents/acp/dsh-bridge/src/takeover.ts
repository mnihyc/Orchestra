/**
 * Takes over the stdio of the process, so that the plugin sits between dsh's ACP server and the real streams.
 *
 * The server asks for `process.stdin` and `process.stdout` when it starts, and the plugin's module is evaluated before
 * any row of the profile starts, so replacing both properties here is enough: the server reads and writes what the
 * channel gives it. Nothing is changed for a process that is not serving the protocol (a help, a version or a
 * configuration dump, which print to stdout and must keep doing so).
 */
import { Channel } from './channel.ts'

const TAKEN = Symbol.for('orch-dsh-bridge.stdio')

/** Flags of dsh's launcher that print to stdout and end; the plugin leaves those runs alone. */
const PRINTING_FLAGS = ['--help', '-h', '--version', '-V', '--dump-config', '--dump-config-schema', '--dump-default-config']

/** Whether this process is one that serves the protocol on its stdio. */
export function servesProtocol(argv: readonly string[], stdinIsTerminal: boolean): boolean {
  if (stdinIsTerminal) return false
  return !argv.some((word) => PRINTING_FLAGS.includes(word))
}

/**
 * Makes the channel for this process and installs it, once. Returns undefined when the process does not serve the
 * protocol, or when another copy of the plugin has already taken the stdio.
 */
export function takeOverStdio(log: (line: string) => void): Channel | undefined {
  const holder = globalThis as unknown as Record<symbol, Channel | undefined>
  if (holder[TAKEN] !== undefined) return undefined
  if (!servesProtocol(process.argv, process.stdin.isTTY === true)) return undefined
  const realStdin = process.stdin
  const realStdout = process.stdout
  const channel = new Channel({
    input: realStdin,
    output: { write: (line) => realStdout.write(line), once: (event, listener) => realStdout.once(event, listener) },
    log,
  })
  Object.assign(channel.serverOutput, { isTTY: false, fd: 1 })
  Object.defineProperty(process, 'stdin', { configurable: true, get: () => channel.serverInput })
  Object.defineProperty(process, 'stdout', { configurable: true, get: () => channel.serverOutput })
  holder[TAKEN] = channel
  return channel
}
