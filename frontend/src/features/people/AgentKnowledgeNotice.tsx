/** Agent knowledge comes from current giver-bounded contributions, never an automatic policy. */
import { useTranslation } from 'react-i18next';

export function AgentKnowledgeNotice() {
  const { t } = useTranslation();
  return <p className="text-sm text-text-muted">{t('agentAccess.giverBound')}</p>;
}
