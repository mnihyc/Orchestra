/**
 * Whether a shell command only reads. Plan mode runs only such commands, and approvals do not ask for them.
 *
 * The check is deliberately narrow: a command passes only when every part of it is a command from the list, and
 * nothing in it can write, run another program, or substitute output. When in doubt it says no.
 */

/** Characters and words that make a segment more than a plain read. */
const WRITES_OR_SUBSTITUTES = /`|\$\(|<\(|>\(|\$\{|\bsudo\b|\beval\b|\bexec\b|\bxargs\b|\btee\b/

/** A redirection that writes somewhere other than the bit bucket or another stream. */
function writesWithRedirect(segment: string): boolean {
  const withoutSafe = segment.replace(/\d?>&\d/g, ' ').replace(/\d?>>?\s*\/dev\/null/g, ' ')
  return />/.test(withoutSafe)
}

/** Flags that turn a read command into a writer. */
function hasWritingFlag(words: string[]): boolean {
  const joined = ` ${words.join(' ')} `
  if (words[0] === 'find') return /\s-(exec|execdir|ok|okdir|delete|fprint|fprintf|fls)\b/.test(joined)
  if (words[0] === 'rg') return /\s--(pre|pre-glob)\b/.test(joined)
  if (words[0] === 'sed') return /\s-[a-zA-Z]*i/.test(joined)
  if (words[0] === 'git') return /\s--(output|ext-diff|exec-path)\b|\s-o\b/.test(joined)
  return false
}

/** Splits a command line into its simple commands at `&&`, `||`, `;`, `|` and newlines. */
export function segmentsOf(command: string): string[] {
  return command
    .split(/&&|\|\||;|\||\n/)
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

/** Drops leading `NAME=value` words and `env`, which only set variables for the command after them. */
function stripEnvironment(words: string[]): string[] {
  let at = 0
  while (at < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[at] ?? '')) at += 1
  if (words[at] === 'env') {
    at += 1
    while (at < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[at] ?? '')) at += 1
  }
  return words.slice(at)
}

/**
 * True when `command` only reads.
 * @param command - the shell command line
 * @param readCommands - first words (or first two words, for `git status`) that only read
 */
export function isReadOnlyCommand(command: string, readCommands: readonly string[]): boolean {
  if (command.trim().length === 0) return false
  const allowed = new Set(readCommands)
  for (const segment of segmentsOf(command)) {
    if (WRITES_OR_SUBSTITUTES.test(segment) || writesWithRedirect(segment)) return false
    const words = stripEnvironment(segment.split(/\s+/))
    if (words.length === 0) continue
    const first = words[0] ?? ''
    const pair = `${first} ${words[1] ?? ''}`.trim()
    if (!allowed.has(first) && !allowed.has(pair)) return false
    if (hasWritingFlag(words)) return false
  }
  return true
}
