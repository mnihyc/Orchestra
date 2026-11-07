/** One native record source for the resource layout and frame fixtures. */
import type { McpServer, Mount, ResourceAccess, Skill, SkillVersion, SkillVersionInfo, SkillVersionInfoPage, UserRef } from '../../src/generated/api-types.ts';
import { ACME, id, RUNS_ON } from './fixtures.ts';

export const SKILL_ID = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a62';

export function extraResourceFixtures(owner: UserRef, access: (kind: string) => ResourceAccess) {
  const home = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };
  const common = { description: 'Shared project files', org_id: ACME.org, home, owner, enabled: true, revision: 1 };
  const mcp: McpServer = { ...common, id: id(0x31, 1), name: 'Project search', address: 'acme/project-search', machine: null, environment: null, imported: null, spec: { transport: 'stdio', command: ['project-search'] }, access: access('mcp') };
  const mount: Mount = { ...common, id: id(0x32, 1), name: 'Reference files', address: 'acme/reference-files', target: '/reference', read_only: true, default_fate: 'as_is', locations: [], locations_hidden: 0, kept: [], access: access('mount') };
  const origin = { kind: 'git' as const, repository: 'https://git.example.test/acme/skills.git', reference: 'main',
    commit: '0123456789abcdef0123456789abcdef01234567', folder: 'pdf-tools', host_id: RUNS_ON.buildHost };
  const latest: SkillVersionInfo = { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a63', number: 2, name: 'pdf-tools', description: 'Reads and fills PDF forms.',
    size: 2048, file_count: 2, archive: { blob: 'ab'.repeat(32), size: 1024 }, origin, created_by: owner, created_at: '2026-10-14T08:00:00Z' };
  const skill: Skill = { id: SKILL_ID, name: 'pdf-tools', description: 'Reads and fills PDF forms.', org_id: ACME.org, owner, home,
    address: 'acme.pdf-tools', enabled: true, latest, versions: 2, origin, draft: null, revision: 1, access: { ...access('skill'), administers: false, admin_only: [], participating: true } };
  const files: SkillVersion = { version: latest, files: [
    { path: 'SKILL.md', size: 1536, sha256: 'cd'.repeat(32), executable: false },
    { path: 'scripts/fill.py', size: 512, sha256: 'ef'.repeat(32), executable: true }] };
  const versions: SkillVersionInfoPage = { items: [latest], next_cursor: null };
  return { mcp, mount, skill, files, versions };
}
