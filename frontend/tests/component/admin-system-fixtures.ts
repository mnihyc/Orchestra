/** Synthetic notifier records shared by system-channel view and API routing tests. */
import type { NotificationChannel, Notifier } from '@/generated/api-types';

export function systemChannel(overrides: Partial<NotificationChannel> = {}): NotificationChannel {
  return { id: '00000000-0000-4000-8100-000000000001', name: 'System relay', kind: 'relay', owner: 'system', config: {},
    secrets_set: ['token'], templates: {}, enabled: true, can_edit: true, enabled_route_count: 0, last_ok_at: null, last_error: null,
    route: { id: null, channel_id: null, channel: null, enabled: false, health: 'never', last_ok_at: null, failed_at: null,
      failure: null, waiting_count: 0, kinds: [], label: null, last_sent_at: null, link: null, min_interval_minutes: null,
      unread_delay_minutes: null, needs: null }, ...overrides };
}

export function systemNotifier(): Notifier {
  return { name: 'relay', title: 'Relay', summary: 'Sends through the configured relay.', links: false, single: true,
    owners: ['system', 'org'], targets: ['person'],
    fields: [{ name: 'token', label: 'Relay token', type: 'secret', required: true, help: null, default: null, choices: null, max_bytes: null }],
    templates: { single: { subject: null, body: '{{title}}' }, digest: { subject: null, body: '{{count}} notifications' } } };
}
