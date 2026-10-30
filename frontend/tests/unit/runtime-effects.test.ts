/** A change form follows the driver's live declaration and warns only against known measured use. */
import { describe, expect, it } from 'vitest';
import { memoryBelowUse, runtimeEffect } from '@/features/settings/runtimeEffects';
import type { RuntimeChanges } from '@/generated/api-types';

const runtime = (live: boolean): RuntimeChanges => ({ live_changes: { resources: live, mounts: live, capabilities: ['kvm', 'docker_socket'], gateway_given: ['port_place', 'port_host', 'port_link'] }, resources: { memory_mb: 256 }, memory_used_bytes: 64 * 1024 * 1024 });

describe('the running unit change form', () => {
  it('says now only for the changes its driver applies live, and keeps the disk exception visible', () => {
    expect(runtimeEffect('env.resources', runtime(true))).toBe('liveResources');
    expect(runtimeEffect('runtime.resources', runtime(false))).toBe('recreateResources');
    expect(runtimeEffect('env.mounts', runtime(true))).toBe('liveMounts');
    expect(runtimeEffect('env.mounts', runtime(false))).toBe('recreate');
    expect(runtimeEffect('env.template_id', runtime(true))).toBe('recreate');
    expect(runtimeEffect('env.resources', { ...runtime(true), pending_reason: 'This clear requires recreation.' })).toBe('pendingResources');
  });
  it('compares the memory draft to current use, without inventing a reading or blocking the save', () => {
    expect(memoryBelowUse({ memory_mb: 32 }, 64 * 1024 * 1024)).toBe(true);
    expect(memoryBelowUse({ memory_mb: 64 }, 64 * 1024 * 1024)).toBe(false);
    expect(memoryBelowUse({ memory_mb: 128 }, 64 * 1024 * 1024)).toBe(false);
    expect(memoryBelowUse({ memory_mb: 32 }, null)).toBe(false);
    expect(memoryBelowUse({}, 64 * 1024 * 1024)).toBe(false);
  });
});
