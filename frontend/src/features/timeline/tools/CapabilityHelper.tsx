/** A delegated result links its ordinary helper, with its private default stated before opening it. */
import { Trans, useTranslation } from 'react-i18next';
import type { UserRef } from '@/generated/api-types';
import { TypedChip } from '@/features/people/NamedChip';
import { UserChip } from '@/ui/UserChip';

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function decoded(value: unknown): unknown {
  if (typeof value !== 'string') return value;
  try { const result: unknown = JSON.parse(value); return result; } catch { return null; }
}
export function capabilityAnswer(value: unknown): unknown {
  const root = decoded(value);
  if (!object(root)) return null;
  if (object(root.structuredContent)) return root.structuredContent;
  if (Array.isArray(root.content)) {
    const text: unknown = root.content.find((part: unknown) => object(part) && part.type === 'text');
    if (object(text)) return decoded(text.text);
  }
  return root;
}
function actor(value: unknown): UserRef | null {
  if (!object(value) || typeof value.id !== 'string' || (value.kind !== 'human' && value.kind !== 'service')
    || typeof value.display_name !== 'string' || typeof value.username !== 'string' || !object(value.avatar)
    || typeof value.avatar.text !== 'string' || typeof value.avatar.color !== 'number') return null;
  return { id: value.id, kind: value.kind, display_name: value.display_name, username: value.username,
    avatar: { text: value.avatar.text, color: value.avatar.color } };
}

export function CapabilityHelper({ result }: { result: unknown }) {
  const { t, i18n } = useTranslation();
  const answer = capabilityAnswer(result);
  if (!object(answer) || !object(answer.helper)) return null;
  const helper = answer.helper;
  if (typeof helper.thread_id !== 'string' || helper.thread_id.length === 0) return null;
  const owner = actor(helper.actor);
  const names = helper.program_label;
  const language = i18n.resolvedLanguage ?? i18n.language;
  const program = object(names) ? names[language] ?? names.en : null;
  return <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted" data-capability-helper>
    <TypedChip reference={{ kind: 'node', id: helper.thread_id, label: t('timeline.capabilityHelper') }} target={`/thread/${helper.thread_id}`} presentation={{ place: 'compact' }} />
    {typeof program === 'string' && program.length > 0 && <span>· <span>{program}</span></span>}
    {owner === null ? <span>· {t('timeline.capabilityHelperPrivate')}</span> : <span className="inline-flex min-w-0 flex-wrap items-center gap-1">
      · <Trans i18nKey="timeline.capabilityHelperOwner" components={{ owner: <UserChip user={owner} size={24} /> }} />
    </span>}
  </div>;
}
