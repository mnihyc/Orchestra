/** Ordinary, synthetic Ask contract rows shared by component and browser acceptance cases. */
import type { AskConversation, AskGetReply, AskMessage, AskModelsReply, ModelOption, ProviderRef } from '../../src/generated/api-types';
import { ACME, id, me, MODEL_ESTIMATE, RUNS_ON } from '../doubles/fixtures';

export const ASK_ID = id(0x72, 1);
export const ASK_USER = id(0x72, 2);
export const ASK_ANSWER = id(0x72, 3);
export const ASK_TURN = id(0x72, 4);
export const AT = '2026-11-08T03:00:00Z';

export function askConversation(more: Partial<AskConversation> = {}): AskConversation {
  return { id: ASK_ID, user_id: me(1, 'ada', 'Ada Lovelace').id, root_org_id: ACME.org, home_org_id: id(0x72, 5), title: 'Release questions',
    state: 'ready', model: null, payer_id: me(1, 'ada', 'Ada Lovelace').id, created_at: AT, last_message_at: AT, expires_at: '2026-12-08T03:00:00Z',
    retention_days: 30, shown: false, admin_inspectable: true, inspecting: false,
    can: { message: true, stop: true, keep: true, delete: true, export: true }, seq: 5, ...more };
}
export function askMessage(more: Partial<AskMessage> = {}): AskMessage {
  return { id: ASK_ANSWER, turn_id: ASK_TURN, role: 'assistant', content: [{ type: 'text', text: 'Read the release notes.' }], citations: [], complete: true, created_at: AT, ...more };
}
export function askSnapshot(more: Partial<AskGetReply> = {}): AskGetReply {
  return { conversation: askConversation(), messages: [askMessage({ id: ASK_USER, role: 'user', content: [{ type: 'text', text: 'What changed?' }] }), askMessage()], previous_cursor: null, ...more };
}
export function askModels(root = ACME.org, more: Partial<AskModelsReply> = {}): AskModelsReply {
  const provider: ProviderRef = { id: RUNS_ON.openAiKey, name: 'Team API', address: 'team-api', kind: 'api', harness: null, harness_kind: null, placement: 'managed', status: 'ready' };
  const model: ModelOption = { ...MODEL_ESTIMATE, model_name: 'ask-readable', display_name: 'Readable API model', parameters: { input_images: true },
    groups: [], option_constraints: {}, allowed: true, reason: null };
  const selection = { provider_id: provider.id, model: model.model_name };
  return { root_org_id: root, default_model: selection, blocked_reason: null, writable: false, models: [
    { selection, provider, model, request_option_schema: null, allowed: true, is_default: true, unavailable_reason: null },
    { selection: { provider_id: provider.id, model: 'ask-excluded' }, provider,
      model: { ...model, model_name: 'ask-excluded', display_name: 'Excluded API model', allowed: false, reason: 'excluded_by_entry' },
      request_option_schema: null, allowed: false, is_default: false, unavailable_reason: 'no_use' },
  ], ...more };
}
