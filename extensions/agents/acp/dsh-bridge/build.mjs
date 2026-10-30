// Bundles the plugin into one ESM file, `dist/orch-dsh-bridge.mjs`, with a SHA-256 beside it. The two files are
// committed (the Rust adapter embeds the bundle), like the generated API types: `node build.mjs` writes them, and
// `node build.mjs --check` builds again somewhere else and fails if either committed file differs.
//
// The plugin has no runtime dependencies: the host (dsh) is reached only through the Cordis context it passes in, so
// the bundle imports nothing but Node's own modules. The bundler is the one the frontend already installs.
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'

const here = path.dirname(fileURLToPath(import.meta.url))
// The bundler writes each module's path, relative to the directory it runs in, into a comment: build from the package's own
// directory whatever the caller's, so that the bytes do not depend on where the command was typed.
process.chdir(here)
const bundlerPath =
  process.env.ORCH_DSH_BRIDGE_BUNDLER ?? path.resolve(here, '../../../../frontend/node_modules/rolldown/dist/index.mjs')
const { rolldown } = await import(pathToFileURL(bundlerPath).href)

const check = process.argv.includes('--check')
const committed = path.join(here, 'dist')
const out = check ? await mkdtemp(path.join(tmpdir(), 'orch-dsh-bridge-check-')) : committed
await mkdir(out, { recursive: true })
const bundle = await rolldown({
  input: path.join(here, 'src/index.ts'),
  platform: 'node',
  external: [/^node:/],
  treeshake: true,
})
await bundle.write({
  file: path.join(out, 'orch-dsh-bridge.mjs'),
  format: 'esm',
  minify: false,
  sourcemap: false,
})
await bundle.close()

const bytes = await readFile(path.join(out, 'orch-dsh-bridge.mjs'))
const digest = createHash('sha256').update(bytes).digest('hex')
const digestLine = `${digest}  orch-dsh-bridge.mjs\n`
if (!check) {
  await writeFile(path.join(out, 'orch-dsh-bridge.mjs.sha256'), digestLine)
  console.log(`orch-dsh-bridge.mjs ${bytes.length} bytes sha256 ${digest}`)
} else {
  const stale = []
  const read = async (name) => readFile(path.join(committed, name)).catch(() => null)
  const bundleNow = await read('orch-dsh-bridge.mjs')
  if (bundleNow === null || !bundleNow.equals(bytes)) stale.push('orch-dsh-bridge.mjs')
  const digestNow = await read('orch-dsh-bridge.mjs.sha256')
  if (digestNow === null || digestNow.toString('utf8') !== digestLine) stale.push('orch-dsh-bridge.mjs.sha256')
  await rm(out, { recursive: true, force: true })
  if (stale.length > 0) {
    console.error(`${stale.join(' and ')} in extensions/agents/acp/dsh-bridge/dist differ from a fresh build of src/: run \`node extensions/agents/acp/dsh-bridge/build.mjs\` and commit the result`)
    process.exit(1)
  }
  console.log(`dist/orch-dsh-bridge.mjs is what src/ builds (sha256 ${digest})`)
}
