/** A human or agent author uses the same identity card wherever its name is shown. */
import type { Author } from '@/generated/api-types';
import { AgentChip } from '@/ui/AgentChip';
import { UserChip } from '@/ui/UserChip';

export function AuthorChip({ author }: { author: Author }) {
  return 'agent' in author ? <AgentChip name={author.agent} thread={author.thread} /> : <UserChip user={author} size="row" />;
}
