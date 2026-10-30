/** The plugin's settings. */
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { DEFAULTS, resolveConfig } from '../src/config.ts'

describe('settings', () => {
  it('has every extra on, and every number a default, when nothing is set', () => {
    const config = resolveConfig(undefined)
    assert.deepEqual(config, DEFAULTS)
    assert.ok(Object.values(config.extras).every(Boolean))
  })

  it('switches one extra off and takes a changed number', () => {
    const config = resolveConfig({ extras: { fork: false }, streaming: { flushMs: 0 }, approvals: { mode: 'bypass' }, hostVersion: '0.2.1-rc.4' })
    assert.equal(config.extras.fork, false)
    assert.equal(config.extras.steering, true)
    assert.equal(config.streaming.flushMs, 0)
    assert.equal(config.streaming.flushChars, DEFAULTS.streaming.flushChars)
    assert.equal(config.approvals.mode, 'bypass')
    assert.equal(config.hostVersion, '0.2.1-rc.4')
  })

  it('names the field of a wrong value', () => {
    assert.throws(() => resolveConfig({ extras: { nothing: true } }), /extras\.nothing is not an extra/)
    assert.throws(() => resolveConfig({ extras: { fork: 'no' } }), /extras\.fork must be true or false/)
    assert.throws(() => resolveConfig({ streaming: { flushMs: -1 } }), /streaming\.flushMs/)
    assert.throws(() => resolveConfig({ approvals: { mode: 'sometimes' } }), /approvals\.mode/)
    assert.throws(() => resolveConfig({ planMode: { readTools: 'read' } }), /planMode\.readTools must be a list of strings/)
    assert.throws(() => resolveConfig([]), /config must be an object/)
  })
})
