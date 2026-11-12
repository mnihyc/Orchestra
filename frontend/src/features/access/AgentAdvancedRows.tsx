/** Independent thread acts use the same Advanced disclosure as the ordinary permission page. */
import { useTranslation } from 'react-i18next';
import type { ShareParts } from '@/generated/api-types';
import { Checkbox } from '@/ui/Checkbox';
import { Disclosure } from '@/ui/Disclosure';
import { ADVANCED, ADVANCED_WORDS, advancedGiven, withAdvanced, type Advanced } from './agentShares';

export function AgentAdvancedRows({ parts, available, disabled = false, onChange }: {
  parts: ShareParts; available: ShareParts; disabled?: boolean;
  onChange: (parts: ShareParts, row: Advanced) => void;
}) {
  const { t } = useTranslation();
  return <Disclosure title={t('access.advanced.title')}>
    {ADVANCED.map((row) => <div key={row} className="flex min-w-0 items-center py-2">
      <Checkbox label={t(ADVANCED_WORDS[row])} checked={advancedGiven(parts, row)}
        disabled={disabled || parts.timeline === 'manage' || !advancedGiven(available, row)}
        onCheckedChange={(checked) => onChange(withAdvanced(parts, row, checked), row)} />
    </div>)}
  </Disclosure>;
}
