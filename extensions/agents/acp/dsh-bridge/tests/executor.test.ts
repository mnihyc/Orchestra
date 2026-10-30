/** The client of Orch's executor, against a stand-in that works in a real directory. */
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, before, describe, it } from 'node:test'
import { Executor, ExecutorError } from '../src/executor.ts'
import { FAKE_TOKEN, FakeExecutor } from './support/fake-executor.ts'

describe('executor client', () => {
  let fake: FakeExecutor
  let executor: Executor
  let dir: string
  before(async () => {
    fake = await FakeExecutor.start()
    executor = new Executor({ url: fake.url, token: FAKE_TOKEN })
    dir = mkdtempSync(path.join(tmpdir(), 'orch-exec-'))
  })
  after(async () => {
    await fake.close()
  })

  it('writes a file and reads it back, whole and by window, however the answer comes', async () => {
    const file = path.join(dir, 'a.txt')
    await executor.write(file, 'one\ntwo\nthree')
    assert.equal(readFileSync(file, 'utf8'), 'one\ntwo\nthree')
    assert.equal(await executor.read(file), 'one\ntwo\nthree')
    assert.equal(await executor.read(file, { offset: 2, limit: 1 }), 'two')
  })

  it('runs a command to its end and returns what it printed and its exit code', async () => {
    const ran = await executor.run('echo hi; echo there >&2; exit 3', dir)
    assert.match(ran.output, /hi/)
    assert.match(ran.output, /there/)
    assert.equal(ran.exitCode, 3)
  })

  it('stops a command when the caller gives up', async () => {
    const controller = new AbortController()
    const running = executor.run('sleep 30', dir, controller.signal)
    await new Promise((resolve) => setTimeout(resolve, 300))
    controller.abort()
    await assert.rejects(running)
  })

  it('refuses a wrong token and says what the executor said of a failure', async () => {
    const wrong = new Executor({ url: fake.url, token: 'wrong' })
    await assert.rejects(wrong.read(path.join(dir, 'a.txt')), (e: Error) => e instanceof ExecutorError && /401/.test(e.message))
    await assert.rejects(executor.read(path.join(dir, 'missing.txt')), (e: Error) => e instanceof ExecutorError && /ENOENT/.test(e.message))
  })

  it('lets a call wait as long as the caller says, and gives up at the usual time otherwise', async () => {
    const short = new Executor({ url: fake.url, token: FAKE_TOKEN }, 300)
    await assert.rejects(short.call('shell', { command: 'sleep 1', cwd: dir }), (e: Error) => e instanceof ExecutorError && /could not be reached/.test(e.message))
    const done = await short.call('shell', { command: 'sleep 1; echo late', cwd: dir }, undefined, 10000)
    assert.match(done.stdout, /late/)
  })

  it('says it could not reach an executor that is not there', async () => {
    await assert.rejects(new Executor({ url: 'http://127.0.0.1:9/x', token: 't' }).read('/x'), (e: Error) => e instanceof ExecutorError && /could not be reached/.test(e.message))
  })
})
