/** Recorded custody metadata for presentation cases; no fake authorization decision or secret. */
import type { AdminSession, AdminToken, AdminTokenActivity, Export, Import, ImportReportRow, Snapshot, UploadInventoryItem, UserRef } from '../../src/generated/api-types';
import { id, MADE_AT, me, RUNS_ON } from './fixtures';

export const reachPerson: UserRef = me(1, 'ada', 'Ada');
export const reachService: UserRef = { ...reachPerson, id: id(1, 91), kind: 'service', username: 'build', display_name: 'Build service' };
export const reachToken = (overrides: Partial<AdminToken> = {}): AdminToken => ({
  info: { id: id(0x51, 1), name: 'Review key', rules: [], scopes: [], node: null, created_at: MADE_AT, last_used_at: null, expires_at: null },
  owner: reachPerson, admin_access: true, ...overrides,
});
export const reachSession = (overrides: Partial<AdminSession> = {}): AdminSession => ({
  id: id(0x52, 1), current: true, user_agent: 'Chrome/130 Linux', ip: '203.0.113.1',
  created_at: MADE_AT, last_used_at: MADE_AT, expires_at: '2030-11-01T12:00:00Z', admin_access: true, ...overrides,
});
export const reachUpload = (overrides: Partial<UploadInventoryItem> = {}): UploadInventoryItem => ({
  id: 'custody:complete', kind: 'completed', state: 'complete', content_id: id(0x53, 1), session_id: null,
  blob: 'a'.repeat(64), uploader: reachPerson, machine: null, name: 'orphan.txt', media_type: 'text/plain', size: 512, received: 512,
  created_at: MADE_AT, updated_at: MADE_AT, expires_at: null, attached_to: [], attachment_count: 0,
  can_open: false, can_delete: true, admin_access: null, ...overrides,
});
export const reachSnapshot = (overrides: Partial<Snapshot> = {}): Snapshot => ({
  id: '00000000-0000-4000-8000-000000000701', thread_id: null,
  environment: { id: RUNS_ON.workspace, name: 'Workspace', address: '@ada/Workspace', owner: reachPerson, placement: 'managed', ports_exposed: false, status: 'running' }, label: 'Workspace snapshot', kind: 'manual', storage: 'blobs',
  repository_head: null, directory: null, expires_at: null, unavailable_reason: null, state: 'complete', selection: { include: ['**'], exclude: [] },
  file_count: 1, total_bytes: 256, missing_count: 0, env_generation: 1, created_by: reachPerson, created_at: MADE_AT, completed_at: MADE_AT,
  admin_access: { place: 'system', private: true }, ...overrides,
});
export const reachExport = (overrides: Partial<Export> = {}): Export => ({
  id: '00000000-0000-4000-8000-000000000702', node: null, request: { what: 'all' }, state: 'ready', job_id: null,
  bytes: 512, counts: null, left_out: [], created_at: MADE_AT, ready_at: MADE_AT, expires_at: null, downloaded_at: null,
  manifest: { made_by: 'Orch', at: MADE_AT, counts: { documents: 1 } }, admin_access: { place: 'system', private: true }, ...overrides,
});
export const reachImport = (overrides: Partial<Import> = {}): Import => ({
  id: '00000000-0000-4000-8000-000000000703', node: null,
  request: { client_id: 'review-import', source: { files: [{ name: 'reviewed.md', blob: 'b'.repeat(64) }] }, destination: {}, options: {}, choices: {} },
  state: 'done', job_id: null, undo_job_id: null, files: ['reviewed.md'], package: null, counts: null,
  actions: { create: 1, replace: 0, keep_both: 0, skip: 0 }, losses: [], failure: null, created_at: MADE_AT, finished_at: MADE_AT, undone_at: null,
  admin_access: { place: 'system', private: true }, ...overrides,
});
export const reachImportRow: ImportReportRow = { position: 0, source_id: id(0x54, 1), kind: 'document', name: 'Reviewed page', path: [],
  action: 'create', outcome: 'created', made: null, message: null, warnings: [], author: null };
export const reachActivity: AdminTokenActivity = { id: id(0x55, 1), at: MADE_AT, operation: 'DocumentGet', selection: 'DocumentGet', facets: [],
  method: 'GET', route: '/documents/{id}', status: 200, error_code: null, replayed: false, duration_ms: 7, request_id: 'review-call', ip: null, admin_access: true };
