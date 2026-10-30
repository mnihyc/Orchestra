/** The hint of a setting, by the key the server names it with, and the hint of a field that a descriptor declares.
 *
 * Neither has an id of the catalogue's list: a setting is worded under `help.settings.<key>` where the catalogue has words for it, and a
 * descriptor brings the words of its own field. The hints of the options of the resources screens are the help feature's (`features/help/Hint.tsx`),
 * so that the help beside a screen lists them. Each opens the same bounded card at every screen width.
 */
import { SettingHelpLink } from '@/features/help/SettingHint';
import { HelpHint } from '@/ui/HelpHint';

/** The hint of a setting, by the key the server names it with, where the catalogue has words for it. */
export function SettingHint({ settingKey, className }: { settingKey: string; className?: string }) {
  return <span className={className}><SettingHelpLink settingKey={settingKey} /></span>;
}

/** The hint of a field that a descriptor declares: the help its descriptor gives, and nothing for one that gives none. */
export function DeclaredHint({ term, help, className }: { term: string; help: string | null | undefined; className?: string }) {
  if (help === null || help === undefined || help === '') return null;
  return <HelpHint term={term} what={help} {...(className === undefined ? {} : { className })} />;
}
