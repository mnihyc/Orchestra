/** An accepted invitation opens only its returned readable target, including a chat-only guest's real messages route. */
import type { InvitationAcceptReply } from '../../generated/api-types.ts';

export function acceptedDestination(reply: InvitationAcceptReply) {
  const node = reply.node;
  if (node !== null && !node.redacted && node.name !== null) {
    switch (node.kind) {
      case 'org': return {to:'/org/$slug',params:{slug:node.id}} as const;
      case 'project': return {to:'/project/$projectId',params:{projectId:node.id}} as const;
      case 'topic': return {to:'/topic/$topicId',params:{topicId:node.id}} as const;
      case 'thread': return {to:'/thread/$threadId',params:{threadId:node.id}} as const;
    }
  }
  if (reply.chat !== null) return {to:'/org/$slug/messages',params:{slug:reply.chat.org_id},search:{chat:reply.chat.id}} as const;
  if (reply.org !== null) return {to:'/org/$slug',params:{slug:reply.org.address}} as const;
  return {to:'/'} as const;
}
