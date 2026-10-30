/** A skill refused with its reason: the API's words for each fault, and beside them what the refusal is about (a file's path, a member of the front matter, a name), which the words never quote. */
import { ApiError } from '@/api/client';
import { ErrorAlert } from '@/ui/ErrorState';

/** What a refusal is about, as the API gives it beside its words. */
export function refusalAbout(error: unknown): string[] {
  if (!(error instanceof ApiError)) return [];
  const about = error.problem.about;
  return Array.isArray(about) ? about.filter((item): item is string => typeof item === 'string' && item.trim() !== '') : [];
}

/** An error beside a skill's form or draft: a refusal says why and names what it is about; any other error is said as usual. */
export function SkillRefusal({ error }: { error: unknown }) {
  const faults = error instanceof ApiError ? error.faults() : [];
  if (faults.length === 0) return <ErrorAlert error={error} />;
  const about = refusalAbout(error);
  return <ErrorAlert error={error} message={<>
    {faults.map((fault) => <span key={`${fault.path}:${fault.message}`} className="block">{fault.message}</span>)}
    {about.length > 0 && <span className="block break-all font-mono text-xs">{about.join(' · ')}</span>}
  </>} />;
}
