/** Tell them: the tick under the field that sends each person named one message from the sharer in the chat the two have, with the link and a line the sharer may write.
 *
 * It is on when a person is added and absent for a group or a role. Without it the people named are told
 * nothing: the thing is simply there for them, in the sidebar and under Shared with me. Where direct chats are
 * off in the org, or the person is not a member of it, the tick is switched off with that reason and the link is
 * there to copy. Telling is a message like any other, made after the naming has stood: if the message cannot
 * be sent, the person is named all the same, and the sharer is told whom it did not reach.
 */
import { useQuery } from '@tanstack/react-query';
import { Copy } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { createChat, sendMessage } from '@/api/chats';
import { settingsQuery } from '@/api/settings';
import { Hint } from '@/features/help/Hint';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Textarea } from '@/ui/Textarea';
import { copyText } from '@/ui/clipboard';

import { type Candidate, useTargetOrg } from './candidates';
import { linkOf } from './target';

/** The longest line a sharer writes to the people they tell. */
const LINE_MAX = 500;

export interface TellState {
  /** Whether the people named are told. */
  on: boolean;
  /** The sharer's own words, beside the link. */
  line: string;
}

/** The people among those staged who can be told: persons, who are members of the org the chats of the tree belong to. */
export function tellable(staged: readonly Candidate[]): Candidate[] {
  return staged.filter((entry) => entry.user !== undefined && !entry.outside);
}

/** Whether direct chats are off in the org, which decides whether anybody can be told. */
export function useDirectOff(target: AccessTarget): boolean {
  const orgId = useTargetOrg(target);
  const settings = useQuery({ ...settingsQuery(orgId ?? ''), enabled: orgId !== null });
  return settings.data?.find((entry) => entry.key === 'chat.direct')?.value === 'off';
}

/**
 * Sends each person one message with the link in the chat the two have, and one message however many things were shared: the sharer's own
 * line or the words that say what was shared, and a link for each. Answers the names of those who could not be told.
 */
export async function tellThem(targets: readonly AccessTarget[], orgId: string, people: readonly Candidate[], line: string, wording: (names: string) => string): Promise<string[]> {
  const links = targets.flatMap((entry) => {
    const url = linkOf(entry);
    return url === null ? [] : [targets.length === 1 ? url : `${entry.name}: ${url}`];
  });
  const failed: string[] = [];
  for (const entry of people) {
    if (entry.user === undefined) continue;
    try {
      const chat = await createChat(orgId, { user_ids: [entry.user.id] }, newActKey());
      const text = [line.trim() === '' ? wording(targets.map((other) => other.name).join(', ')) : line.trim(), ...links].join('\n');
      await sendMessage(chat.id, { client_id: newActKey(), parts: [{ type: 'text', text }] });
    } catch {
      failed.push(entry.name);
    }
  }
  return failed;
}

export function TellThem({ target, staged, state, onChange }: { target: AccessTarget; staged: readonly Candidate[]; state: TellState; onChange: (next: TellState) => void }) {
  const { t } = useTranslation();
  const off = useDirectOff(target);
  const people = staged.filter((entry) => entry.user !== undefined);
  const members = tellable(staged);
  if (people.length === 0) return null;
  const url = linkOf(target);
  const reason = off ? t('access.tell.off') : members.length === 0 ? t('access.tell.outside', { names: people.map((entry) => entry.name).join(', ') }) : null;
  const names = members.map((entry) => entry.name).join(', ');
  const copy = async () => {
    if (url !== null) await copyText(url, t('common.linkCopied'));
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Checkbox
          label={t('access.tell.label', { names: reason === null ? names : people.map((entry) => entry.name).join(', ') })}
          hint={reason ?? undefined}
          help={<Hint id="share-tell" />}
          checked={reason === null && state.on}
          disabled={reason !== null}
          onCheckedChange={(on) => onChange({ ...state, on })}
        />
        {reason !== null && url !== null && (
          <Button size="sm" icon={<Copy aria-hidden />} onClick={() => void copy()}>
            {t('access.footer.copyLink')}
          </Button>
        )}
      </div>
      {reason === null && state.on && (
        <Textarea label={t('access.tell.line')} optional rows={2} maxLength={LINE_MAX} value={state.line} onChange={(event) => onChange({ ...state, line: event.target.value })} />
      )}
    </div>
  );
}
