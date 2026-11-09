/** Shared native group types, generated from the same contract the existing transport serves. */
import type { ChatUpdateBody, OrgChatCreateBody } from '@/generated/api-types';
export type { Chat, ChatVisibility } from '@/generated/api-types';
export type GroupUpdateBody = ChatUpdateBody;
export type GroupCreateFields = Pick<OrgChatCreateBody, 'home_id' | 'visibility'>;
