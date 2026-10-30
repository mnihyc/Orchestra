/** What the fake API says of a person's own settings: their password, second step, sessions, tokens and the state the interface keeps; and of an org's settings, the settings of a node, channels and destinations.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same
 * shapes, the same refusals and the same members beside them. The code an authenticator shows is the
 * same for everybody (`AUTHENTICATOR_CODE`), so that a test can type it where a person reads it off
 * their phone; a code that is mailed is read from the outbox, as the people of the fake system would
 * read their mail.
 */
import { randomUUID } from 'node:crypto';
import type { ApiTokenInfo, AuditEntry, Destination, Holdings, Me, Member, MemberMovePlan, NodeRef, NotificationChannel, Notifier, ResolvedSetting, Role, SecretInfo, ServiceAccount, SessionInfo, SettingDef, Suspension, UserRef, ResolvedSetting as SettingWithParts } from '../../src/generated/api-types.ts';
import { resolvedParts } from './day100-settings.ts';
import { SETTINGS_REGISTRY } from '../../src/generated/settings-registry.ts';
import type { RecoveryCodeBatch, RecoveryCodeStatus } from '../../src/generated/api-types.ts';
import { ACME, id, RUNS_ON, USUAL_MODEL, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

// ---------------------------------------------------------------- the registry, as far as the double plays it

type Json = unknown;

interface Key {
  def: SettingDef & { group: string };
  /** The kinds of place the key can be set at, from the top. */
  at: readonly string[];
}

const ORG_DOWN = ['org', 'project', 'topic', 'thread'];
const ORG_TO_TOPIC = ['org', 'project', 'topic'];
const WITH_SYSTEM = ['system', 'org', 'project', 'topic'];

function key(
  name: string,
  title: string,
  description: string,
  group: string,
  kind: 'parameter' | 'policy',
  at: readonly string[],
  schema: Json,
  fallback: Json,
  merge: SettingDef['merge'] = 'replace',
  display?: SettingDef['display'],
  example: unknown = fallback,
): Key {
  // The server names every key it ships under the application's catalogue, as the registry does.
  return { def: { key: name, title, description, group, schema, class: kind, scopes: [...at], merge, default: fallback, example, ...(display === undefined ? {} : { display }), text_key: `day100.builtin.${name}`, keywords: [] }, at };
}

const CHOICE = (...values: string[]) => ({ type: 'string', enum: values });
const SET_OF = (...values: string[]) => ({ type: 'array', items: CHOICE(...values), uniqueItems: true });

/** A selection of the real registry that holds every shape of value the editor draws. */
const REGISTRY: readonly Key[] = [
  key('switch.batch_threads', 'Threads per batch', 'Threads in one reviewed switch batch.', 'switching', 'policy', ['system'], {type:'integer',minimum:1}, 200, 'replace', {unit:'raw',zero:null}, 500),
  key('cleanup.unused_keep_days', 'Clean unused caches', 'Remove unused images and tool versions after this many days. Zero switches automatic cleanup off.', 'retention', 'policy', ['system', 'host'], {type:'integer',minimum:0,'x-unit':'days'}, 7, 'replace', {unit:'days',zero:'off'}, 14),
  key('cleanup.release_keep_count', 'Keep unused releases', 'Unused releases kept in addition to current and previous. Zero switches pruning off and keeps all.', 'retention', 'policy', ['system'], {type:'integer',minimum:0}, 3, 'replace', {unit:'raw',zero:'off'}, 5),
  ...SETTINGS_REGISTRY.filter((def) => def.key.startsWith('calls.')).map((def): Key => {
    if (typeof def.group !== 'string') throw new Error('A call policy has its declared settings group');
    return { def: { ...def, group: def.group, ...(def.key === 'calls.repeat_limit' ? { display: { unit: 'raw', zero: 'off' } as const } : {}) }, at: def.scopes };
  }),
  ...SETTINGS_REGISTRY.filter((def) => def.key.startsWith('search.semantic.') || def.key === 'search.word_tuning').map((def): Key => {
    if (typeof def.group !== 'string') throw new Error('A search policy has its declared settings group');
    return { def: { ...def, group: def.group }, at: def.scopes };
  }),
  key('agent.model', 'Model', 'The model that threads here run on, by its routing name.', 'agent', 'parameter', ORG_DOWN, { type: 'string' }, null),
  key('agent.default_provider_id', 'Provider for new threads', 'The provider that a new thread here starts on, by its identifier.', 'agent', 'parameter', ORG_TO_TOPIC, { type: 'string' }, null),
  key('agent.default_agent_id', 'Agent profile for new threads', 'The agent profile that a new thread here starts with, by its identifier.', 'agent', 'parameter', ORG_TO_TOPIC, { type: 'string' }, null),
  key('agent.effort', 'Effort', 'How much effort the model puts into a turn, where it takes the choice.', 'agent', 'parameter', ORG_DOWN, { type: 'string' }, null),
  key('collab.default_delivery', 'Delivery of messages', 'What a message sent to a thread that is busy does: waits its turn, goes into the turn, or stops it and goes first.', 'agent', 'parameter', ORG_DOWN, CHOICE('queue', 'steer', 'interrupt'), 'queue'),
  key('agent.instructions', 'Instructions', 'Standing instructions given to the agent with every session.', 'agent', 'parameter', ORG_DOWN, { type: 'string', maxLength: 20000 }, '', 'append'),
  key('workspace.ignore', 'Ignored paths', 'Patterns excluded from the workspace index.', 'workspace', 'parameter', ORG_TO_TOPIC, { type: 'array', items: { type: 'string' } }, [], 'union'),
  key('agent.features', 'Agent features', 'Which optional features of agents are switched on, by feature.', 'agent', 'policy', WITH_SYSTEM, { type: 'object', additionalProperties: { type: 'boolean' } }, {}, 'map'),
  key('orch_tools.enabled', 'Orch tools', 'Which of Orch’s own tools an agent is offered.', 'agent', 'parameter', ORG_DOWN, SET_OF('list_shared', 'read_shared', 'search_shared', 'edit_document', 'send_message', 'announce'), ['list_shared', 'read_shared', 'search_shared', 'edit_document', 'send_message', 'announce']),
  key('approval.mode', 'Approval mode', 'How the harness decides permission requests inside its existing environment.', 'confinement', 'parameter', ORG_DOWN, CHOICE('ask', 'on_request', 'auto', 'bypass'), 'on_request'),
  key('approval.allowed_modes', 'Allowed approval modes', 'The native permission modes that may be chosen here.', 'confinement', 'policy', ['system', 'org', 'project', 'topic'], SET_OF('ask', 'on_request', 'auto', 'bypass'), ['ask', 'on_request', 'auto', 'bypass'], 'intersect'),
  key('sandbox.mode', 'Sandbox', 'What the harness’s own sandbox lets an agent’s commands do.', 'confinement', 'parameter', ORG_DOWN, CHOICE('read_only', 'workspace', 'open'), 'workspace'),
  key('binding.prefer', 'Preferred arrangements', 'The arrangement types that are tried first, in this order.', 'confinement', 'parameter', WITH_SYSTEM, SET_OF('separated', 'key_separated', 'credential_shared', 'handed_over', 'together'), null),
  key('binding.allowed', 'Allowed arrangements', 'The arrangement types that may be used here at all.', 'confinement', 'policy', WITH_SYSTEM, SET_OF('separated', 'key_separated', 'credential_shared', 'handed_over', 'together'), null, 'intersect'),
  key('models.allowed', 'Allowed models', 'Patterns naming the models that may be used here at all.', 'models', 'policy', WITH_SYSTEM, { type: 'array', items: { type: 'string' } }, null, 'intersect'),
  key('env.thread_default', 'New threads', 'Whether a new thread joins the resolved default environment or is given one of its own.', 'environment', 'parameter', WITH_SYSTEM, CHOICE('shared', 'fresh'), 'shared'),
  key('env.default_id', 'Default environment', 'The environment new threads join when they share one and ask for no other. The creator must manage it. Clear the choice to inherit the place above.', 'environment', 'parameter', WITH_SYSTEM, { type: 'string', format: 'uuid' }, null),
  key('tags.max_per_place', 'Tags on a place', 'How many tags one place carries at most.', 'limits', 'policy', ['system', ...ORG_DOWN], { type: 'integer', minimum: 0 }, 5),
  key('env.idle_stop_minutes', 'Idle stop', 'Stop a managed environment after this many minutes without activity; 0 never stops it.', 'environment', 'parameter', ORG_TO_TOPIC, { type: 'integer', minimum: 0, 'x-unit': 'minutes' }, 1440),
  key('env.variables', 'Variables', 'Variables given to the environment, by name.', 'environment', 'parameter', ORG_TO_TOPIC, { type: 'object', additionalProperties: { type: 'string' } }, {}, 'map'),
  key('terminal.enabled', 'Shells', 'Whether interactive shells may be opened.', 'shells', 'policy', WITH_SYSTEM, { type: 'boolean' }, true),
  key('terminal.max_per_person', 'Shells per person', 'How many shells one person may have open at once in one environment.', 'shells', 'policy', WITH_SYSTEM, { type: 'integer', minimum: 1 }, 4),
  key('docs.max_bytes', 'Document size', 'The most text one document here may hold, in bytes.', 'chat_documents', 'policy', WITH_SYSTEM, { type: 'integer', minimum: 65536, 'x-unit': 'bytes' }, 2097152),
  key('invitations.forms', 'Invitation forms', 'The forms of invitation that may be made for an org of this tree.', 'joining', 'policy', ['system', 'org'], SET_OF('link', 'email'), ['link', 'email']),
  key('invitations.default_hours', 'Invitation life', 'For how many hours an invitation is good when its inviter says nothing.', 'joining', 'policy', ['system', 'org'], { type: 'integer', minimum: 1, 'x-unit': 'hours' }, 168),
  key('invitations.max_hours', 'Longest invitation', 'The most hours an invitation may be good for.', 'joining', 'policy', ['system', 'org'], { type: 'integer', minimum: 1, 'x-unit': 'hours' }, 720),
  key('invitations.max_uses', 'Uses of a shared link', 'How many people one shared link may bring in at most.', 'joining', 'policy', ['system', 'org'], { type: 'integer', minimum: 1, maximum: 1000 }, 100),
  key('auth.email_code', 'Code by mail', 'What a code sent to a person’s confirmed address may be when they sign in.', 'signing_in', 'policy', ['system', 'org'], SET_OF('sign_in', 'second_step'), []),
  key('auto.triggers', 'Triggers', 'When automatic titles and descriptions are written again.', 'automation', 'parameter', WITH_SYSTEM, { type: 'object', properties: { on: SET_OF('turn_completed', 'milestone', 'interval'), min_interval_secs: { type: 'integer', minimum: 0, 'x-unit': 'seconds' } }, required: ['on', 'min_interval_secs'], additionalProperties: false }, { on: ['turn_completed', 'milestone'], min_interval_secs: 120 }),
  key('retention.archive_idle_days', 'Archive idle topics', 'Archive a topic after this many days without activity; 0 never does.', 'retention', 'policy', WITH_SYSTEM, { type: 'integer', minimum: 0, 'x-unit': 'days' }, 0),
  key('sharing.allow_external', 'Owners share outside', 'Whether the owners of a thing here may themselves name somebody from outside its root org on its page.', 'collaboration', 'policy', WITH_SYSTEM, { type: 'boolean' }, true),
  // The fake world's orgs keep their trees by name, the order every test that reads the tree was written against; the
  // real registry's own default is the most recently changed first.
  key('navigation.order', 'Order of the tree', 'The order in which the org’s tree is shown to whoever follows the org’s own.', 'collaboration', 'parameter', ['system', 'org'], { type: 'object', properties: { by: CHOICE('custom', 'newest', 'changed', 'name'), custom: { type: 'object', additionalProperties: { type: 'array', items: { type: 'string' }, uniqueItems: true } } }, required: ['by'], additionalProperties: false }, { by: 'name' }),
  key('thread.cwd', 'Start directory', 'Where the agent starts inside the thread’s directory, as a relative path.', 'environment', 'parameter', ['thread'], { type: 'string' }, ''),
];

// ---------------------------------------------------------------- the kinds of channel the double has

const field = (name: string, label: string, type: Notifier['fields'][number]['type'], required: boolean, help: string | null, more: Partial<Notifier['fields'][number]> = {}): Notifier['fields'][number] => ({
  name,
  label,
  type,
  required,
  help,
  default: null,
  choices: null,
  max_bytes: null,
  ...more,
});

const TEXTS = { single: { subject: null, body: '{{title}}' }, digest: { subject: null, body: '{{count}} new' } };

/** The kinds of channel and of destination this system has: three built in, and one invented that only a destination can be. */
const NOTIFIERS: readonly Notifier[] = [
  {
    name: 'email',
    title: 'Mail server',
    summary: 'Sends notifications to the confirmed addresses of the members who switch it on.',
    fields: [
      field('host', 'Host', 'text', true, 'The name of the mail server.'),
      field('port', 'Port', 'text', true, null, { default: '587' }),
      field('security', 'How it is secured', 'choice', true, null, { default: 'starttls', choices: [{ value: 'starttls', label: 'STARTTLS' }, { value: 'tls', label: 'TLS' }, { value: 'none', label: 'Not secured' }] }),
      field('user', 'User', 'text', false, null),
      field('password', 'Password', 'secret', false, 'Written and never shown.'),
      field('from', 'Sends from', 'text', true, 'The address mail comes from.'),
    ],
    links: false,
    owners: ['system', 'org'],
    single: true,
    targets: ['person'],
    templates: TEXTS,
  },
  {
    name: 'telegram',
    title: 'Telegram bot',
    summary: 'A bot that members connect their chat to.',
    fields: [field('token', 'Bot token', 'secret', true, 'From the bot’s own page; its name is read from it.')],
    links: true,
    owners: ['system', 'org'],
    single: true,
    targets: ['person'],
    templates: TEXTS,
  },
  {
    name: 'push',
    title: 'Push',
    summary: 'Tells a browser that takes push.',
    fields: [],
    links: false,
    owners: ['system', 'org'],
    single: true,
    targets: ['person'],
    templates: TEXTS,
  },
  {
    name: 'webhook',
    title: 'Webhook',
    summary: 'Sends a signed POST to an address.',
    fields: [field('url', 'Address', 'text', true, 'Where the POST goes.'), field('secret', 'Secret', 'secret', false, 'Signs what is sent.')],
    links: false,
    owners: ['system', 'org', 'user'],
    single: false,
    targets: ['person', 'destination'],
    templates: TEXTS,
  },
  {
    name: 'chatroom',
    title: 'Team room',
    summary: 'A room in the team’s chat tool, for what happens here.',
    fields: [field('room', 'Room', 'text', true, 'The name of the room.'), field('notify_all', 'Notify everybody in the room', 'boolean', false, null)],
    links: false,
    owners: ['org'],
    single: false,
    targets: ['destination'],
    templates: TEXTS,
  },
];

/** The code every authenticator app of the fake system shows. */
export const AUTHENTICATOR_CODE = '135790';

/** The secret of the authenticator app a person is setting up. */
const AUTHENTICATOR_SECRET = 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP';

/** The keys the interface may keep for a person: the nine the server admits. */
const STATE_KEYS = ['help', 'layouts', 'viewers', 'timeline', 'sidebar', 'project_views', 'keys', 'reactions', 'inbox'] as const;

/** A person's keys in the shape the server keeps them: the switch, and for each action its keys as text or none. */
function keysFault(body: Record<string, unknown>): string | null {
  if (body.single_key !== undefined && typeof body.single_key !== 'boolean') return 'single_key: is true or false';
  const bindings = body.bindings ?? {};
  if (typeof bindings !== 'object' || Array.isArray(bindings)) return 'bindings: is a map of actions to keys';
  for (const [id, keys] of Object.entries(bindings)) {
    if (keys !== null && (typeof keys !== 'string' || keys.trim() === '')) return `bindings.${id}: are no keys`;
  }
  return null;
}

const asText = (value: unknown): string => (typeof value === 'string' ? value : '');

/** What the channels of the system are kept under, beside those of the orgs. */
const SYSTEM_CHANNELS = 'system';

const ADDRESS = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

interface TokenHeld {
  info: ApiTokenInfo;
  secret: string;
}

/** Somebody held out of an org, with why and by whom. */
interface SuspendedHere {
  suspension: Suspension;
}

/** A resource a person made in the tree, as the dialog that ends a membership lists it. */
interface MadeResource {
  object: { kind: string; id: string; name: string };
  shared_with: number;
  threads: number;
  personal_sign_in: boolean;
}

const ROLES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];

/** The part of the fake API about settings. */
export class Settings implements Part {
  readonly world: World;
  private sessions = new Map<string, SessionInfo[]>();
  private tokens = new Map<string, TokenHeld[]>();
  private state = new Map<string, Record<string, unknown>>();
  /** Who has asked for the secret of an authenticator app and not switched it on yet. */
  private pendingSecret = new Set<string>();
  /** Who is held out of which org: org identifier, then username. */
  private suspended = new Map<string, Map<string, SuspendedHere>>();
  /** What a person made in the tree, as resources, by username. */
  private made = new Map<string, MadeResource[]>();

  constructor(world: World) {
    this.world = world;
  }

  private invalid(path: string, message: string): never {
    throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
  }

  // ---------------------------------------------------------------- the person's own sign-in

  private sessionTemplates(person: Person): SessionInfo[] {
    let list = this.sessions.get(person.me.username);
    if (list === undefined) {
      list = [
        {
          id: randomUUID(),
          current: true,
          user_agent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36',
          ip: '203.0.113.7',
          created_at: '2026-10-11T08:00:00.000Z',
          last_used_at: '2026-10-12T10:00:00.000Z',
          expires_at: '2026-11-10T08:00:00.000Z',
        },
        {
          id: randomUUID(),
          current: false,
          user_agent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
          ip: '198.51.100.23',
          created_at: '2026-10-05T17:00:00.000Z',
          last_used_at: '2026-10-09T12:30:00.000Z',
          expires_at: '2026-11-04T17:00:00.000Z',
        },
      ];
      this.sessions.set(person.me.username, list);
    }
    return list;
  }

  private sessionsOf(person: Person): SessionInfo[] {
    const templates = this.sessionTemplates(person);
    const refs = this.world.browserSessionRefs(person);
    const current = templates.find((entry) => entry.current);
    if (refs === null || current === undefined) return templates;
    // Preserve the declared older-device fixture, while real browser sessions
    // have their actual IDs/currentness in the mode event and this projection.
    const ids = new Set(refs.map((entry) => entry.id));
    return [...refs.map((entry) => ({ ...current, ...entry })), ...templates.filter((entry) => !entry.current && !ids.has(entry.id))];
  }

  /** Refuses a password that is not the person's, as the real API does for a signed-in session. */
  private checkPassword(person: Person, password: unknown): void {
    if (!person.me.has_password) return;
    if (asText(password) !== person.password) {
      throw new Refusal(401, 'auth.invalid_credentials', 'the password is not right');
    }
  }

  private setMe(person: Person, change: Partial<Me>): Me {
    person.me = { ...person.me, ...change };
    return person.me;
  }

  /** A change to the person that is more than the look of the page: the address, which needs the password. Anything else is the world's. */
  updateMe(person: Person, body: Record<string, unknown>): Answer | null {
    if (body.email === null) {
      this.checkPassword(person, body.current_password);
      if (person.me.email_code_second_step) this.invalid('email','switch off the mailed second step before clearing the address');
      const passwordWorks = person.me.has_password && (this.world.meta.local_login || person.me.is_system_admin);
      const identityWorks = person.me.identities.some((identity) => this.world.meta.identity_providers.some((provider) => provider.key === identity.idp_key));
      if (!passwordWorks && !identityWorks) this.invalid('email','keep another allowed way to sign in before clearing the address');
      for (const [token, username] of this.world.confirmations) if (username === person.me.username) this.world.confirmations.delete(token);
      for (const [token, username] of this.world.resets) if (username === person.me.username) this.world.resets.delete(token);
      person.mailed=null;
      this.setMe(person,{email:null,email_pending:null,email_verified:false});
      return {status:200,body:person.me};
    }
    if (typeof body.email !== 'string') return null;
    const email = body.email.trim();
    if (!ADDRESS.test(email)) this.invalid('email', 'is not an address');
    this.checkPassword(person, body.current_password);
    if (email.toLowerCase() === (person.me.email ?? '').toLowerCase()) this.invalid('email', 'is the address already');
    for (const other of this.world.people.values()) {
      if (other.me.id !== person.me.id && other.me.email?.toLowerCase() === email.toLowerCase()) {
        throw new Refusal(409, 'conflict.exists', 'the address is taken');
      }
    }
    if (this.world.meta.mail) {
      // Nothing changes until the new address is confirmed; the old one stays the one that codes go to.
      const secret = `confirm-${randomUUID()}`;
      this.world.confirmations.set(secret, person.me.username);
      this.world.outbox.push({ to: person.me.username, kind: 'confirm', secret });
      this.setMe(person, { email_pending: email });
    } else {
      this.setMe(person, { email, email_verified: false, email_pending: null });
    }
    return { status: 200, body: person.me };
  }

  private changePassword(person: Person, body: Record<string, unknown>): Answer {
    this.checkPassword(person, body.current_password);
    const next = asText(body.new_password);
    if (Array.from(next).length < 10) this.invalid('new_password', 'must be at least 10 characters');
    person.password = next;
    // Every other session ends with it.
    this.world.revokeBrowserSessions(person);
    this.sessions.set(person.me.username, this.sessionTemplates(person).filter((session) => session.current));
    return { status: 204 };
  }

  private totpSetup(person: Person, body: Record<string, unknown>): Answer {
    this.checkPassword(person, body.password);
    this.pendingSecret.add(person.me.username);
    return {
      status: 200,
      body: {
        secret: AUTHENTICATOR_SECRET,
        otpauth_url: `otpauth://totp/Orch%20%28double%29:${person.me.username}?secret=${AUTHENTICATOR_SECRET}&issuer=Orch%20%28double%29`,
      },
    };
  }

  private totpEnable(person: Person, body: Record<string, unknown>): Answer {
    if (!this.pendingSecret.has(person.me.username)) this.invalid('code', 'no authenticator app is being set up');
    if (asText(body.code).replace(/\s/g, '') !== AUTHENTICATOR_CODE) {
      throw new Refusal(401, 'auth.second_step_invalid', 'that code did not work');
    }
    this.pendingSecret.delete(person.me.username);
    person.totp = AUTHENTICATOR_CODE;
    this.setMe(person, { totp_enabled: true });
    const recovery_codes = this.replaceCodes(person);
    return { status: 200, body: { me: person.me, recovery_codes }, headers: { 'Cache-Control': 'no-store' } };
  }

  private recoveryStatus(person: Person): RecoveryCodeStatus {
    return { remaining: person.totp === null ? 0 : person.recoveryCodes.length, created_at: person.recoveryCreatedAt,
      methods: [...(person.totp === null ? [] : ['totp' as const]), ...(person.recoveryCodes.length > 0 ? ['recovery_code' as const] : []), ...(person.me.email_code_second_step ? ['email_code' as const] : [])], recent_recovery: false };
  }

  private replaceCodes(person: Person): RecoveryCodeBatch {
    person.recoveryCodes = Array.from({ length: 10 }, () => randomUUID().replace(/-/g, '').slice(0, 20).toUpperCase().match(/.{4}/g)?.join('-') ?? '');
    person.recoveryCreatedAt = new Date().toISOString();
    return { codes: [...person.recoveryCodes], status: this.recoveryStatus(person) };
  }

  private recoveryRegenerate(person: Person, body: Record<string, unknown>): Answer {
    const proof = typeof body.proof === 'object' && body.proof !== null ? body.proof as Record<string, unknown> : {};
    this.checkPassword(person, proof.password);
    if (person.totp === null) this.invalid('', 'set up an authenticator first');
    this.verifyStep(person, proof);
    return { status: 200, body: this.replaceCodes(person), headers: { 'Cache-Control': 'no-store' } };
  }

  private verifyStep(person: Person, body: Record<string, unknown>): Answer {
    const saved = asText(body.recovery_code);
    const rightRecovery = person.totp !== null && saved !== '' && person.recoveryCodes.some((code) => this.normal(code) === this.normal(saved));
    const rightApp = person.totp !== null && asText(body.totp) === person.totp;
    const rightMail = person.me.email_code_second_step && person.mailed !== null && this.normal(asText(body.email_code)) === this.normal(person.mailed);
    if (!rightRecovery && !rightApp && !rightMail) throw new Refusal(401, 'auth.second_step_invalid', 'that code did not work or was used already');
    if (rightRecovery) person.recoveryCodes = person.recoveryCodes.filter((code) => this.normal(code) !== this.normal(saved));
    if (rightMail) person.mailed = null;
    return { status: 200, body: person.me };
  }

  private totpDisable(person: Person, body: Record<string, unknown>): Answer {
    this.checkPassword(person, body.password);
    const saved = person.recoveryCodes.find((code) => this.normal(code) === this.normal(asText(body.code)));
    if (person.totp === null || asText(body.code).replace(/\s/g, '') !== person.totp && saved === undefined) {
      throw new Refusal(401, 'auth.second_step_invalid', 'that code did not work');
    }
    person.totp = null;
    person.recoveryCodes = [];
    person.recoveryCreatedAt = null;
    this.setMe(person, { totp_enabled: false });
    return { status: 204 };
  }

  private mailCodeSend(person: Person, body: Record<string, unknown>): Answer {
    this.checkPassword(person, body.password);
    if (person.me.email === null || !person.me.email_verified) {
      throw new Refusal(403, 'auth.email_unverified', 'there is no confirmed address to send to');
    }
    person.mailed = `${randomCode()}-${randomCode()}`;
    this.world.outbox.push({ to: person.me.username, kind: 'code', secret: person.mailed });
    return { status: 202 };
  }

  private normal(code: string): string {
    return code.replace(/[\s-]/g, '').toUpperCase();
  }

  private mailCodeEnable(person: Person, body: Record<string, unknown>): Answer {
    if (!person.me.email_code.includes('second_step')) {
      throw new Refusal(403, 'auth.email_code_not_allowed', 'your orgs do not allow a mailed code as a second step');
    }
    if (person.mailed === null || this.normal(person.mailed) !== this.normal(asText(body.code))) {
      throw new Refusal(401, 'auth.second_step_invalid', 'that code did not work');
    }
    person.mailed = null;
    this.setMe(person, { email_code_second_step: true });
    return { status: 204 };
  }

  private mailCodeDisable(person: Person, body: Record<string, unknown>): Answer {
    this.checkPassword(person, body.password);
    const code = asText(body.code);
    const right = (person.mailed !== null && this.normal(person.mailed) === this.normal(code)) || (person.totp !== null && code.replace(/\s/g, '') === person.totp);
    if (!right) throw new Refusal(401, 'auth.second_step_invalid', 'that code did not work');
    person.mailed = null;
    this.setMe(person, { email_code_second_step: false });
    return { status: 204 };
  }

  // ---------------------------------------------------------------- an org's members

  private personById(id: string): Person {
    const found = [...this.world.people.values()].find((candidate) => candidate.me.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such person');
    return found;
  }

  /** The person a path names: an identifier, or `me`. */
  private personOf(caller: Person, id: string): Person {
    return id === 'me' ? caller : this.personById(id);
  }

  private rootOf(orgId: string): string {
    const root = this.world.places.rootOf(orgId);
    if (root === null) throw new Refusal(404, 'not_found', 'no such org');
    return root;
  }

  /** The suspension that holds somebody out of an org, or out of one above it. */
  private suspensionIn(orgId: string, username: string): Suspension | null {
    for (const row of this.world.places.pathTo(orgId)) {
      const held = this.suspended.get(row.id)?.get(username);
      if (held !== undefined) return held.suspension;
    }
    return null;
  }

  /** The nodes under a place, itself not counted. */
  private under(placeId: string): { id: string; kind: NodeRef['kind'] }[] {
    const root = this.rootOf(placeId);
    const rows = [...(this.world.trees.get(root)?.nodes.values() ?? [])];
    const inside = (id: string) => this.world.places.pathTo(id).some((row) => row.id === placeId);
    return rows.filter((row) => row.id !== placeId && !row.deleted && inside(row.id)).map((row) => ({ id: row.id, kind: row.kind }));
  }

  /** What a person owns in a tree, by kind: what ending their membership would pass on. */
  private ownedBy(username: string, root: string): Holdings['nodes'] {
    const counts = { orgs: 0, projects: 0, topics: 0, threads: 0, documents: 0 };
    for (const row of this.world.trees.get(root)?.nodes.values() ?? []) {
      if (row.deleted || this.world.places.metaOf(row.id).owner !== username || row.id === root) continue;
      if (row.kind === 'org') counts.orgs += 1;
      else if (row.kind === 'project') counts.projects += 1;
      else if (row.kind === 'topic') counts.topics += 1;
      else counts.threads += 1;
    }
    return counts;
  }

  /** The members of an org: the rows are the places', and this adds who is held out and what a person holds by name elsewhere. */
  private memberList(person: Person, orgId: string, query: URLSearchParams): Answer {
    const page = this.world.places.memberList(person, orgId, query);
    const manages = this.world.places.access(person, orgId).permissions.includes('members.manage');
    const items = page.items.map((member) => ({
      ...member,
      also: this.also.get(member.user.username) ?? 0,
      // Only whoever manages the members is told who is held out.
      suspended: manages && this.suspensionIn(orgId, member.user.username) !== null,
    }));
    return { status: 200, body: { ...page, items } };
  }

  /** Roles some people hold by name elsewhere in the tree, besides their domain: what the list counts. */
  private also = new Map<string, number>();

  private suspend(person: Person, orgId: string, userId: string, body: Record<string, unknown>): Answer {
    this.world.places.need(person, orgId, 'members.manage');
    const target = this.personOf(person, userId);
    if (target.me.id === person.me.id) throw new Refusal(403, 'authz.denied', 'you cannot hold yourself out');
    if (target.me.is_system_admin) throw new Refusal(403, 'authz.denied', 'a system admin cannot be suspended in an org');
    if (this.world.places.metaOf(orgId).owner === target.me.username || this.world.places.metaOf(this.rootOf(orgId)).owner === target.me.username) {
      throw new Refusal(403, 'authz.denied', 'an owner of the org cannot be suspended in it');
    }
    const suspension: Suspension = {
      user: userRef(target.me),
      outside: this.world.places.roles.get(this.rootOf(orgId))?.has(target.me.username) !== true,
      reason: asText(body.reason).trim(),
      by: userRef(person.me),
      at: new Date().toISOString(),
      owns: Object.values(this.ownedBy(target.me.username, this.rootOf(orgId))).reduce((sum, count) => sum + count, 0),
    };
    const here = this.suspended.get(orgId) ?? new Map<string, SuspendedHere>();
    here.set(target.me.username, { suspension });
    this.suspended.set(orgId, here);
    return { status: 200, body: suspension };
  }

  private movePlan(person: Person, orgId: string, userId: string, body: Record<string, unknown>): MemberMovePlan {
    const target = this.personById(userId);
    const root = this.rootOf(orgId);
    const held = this.world.places.roles.get(root)?.get(target.me.username);
    if (held === undefined) throw new Refusal(404, 'not_found', 'that person is no member here');
    const to = asText(body.to);
    if (this.world.places.rootOf(to) !== root || this.world.places.rowOf(to)?.kind !== 'org') {
      throw new Refusal(409, 'node.wrong_parent', 'a member is moved to an org of the same tree');
    }
    if (held.domain === to) throw new Refusal(409, 'member.same_domain', 'that org is already their home');
    const from = this.world.places.rowOf(held.domain);
    const destination = this.world.places.rowOf(to);
    if (from === undefined || destination === undefined) throw new Refusal(404, 'not_found', 'no such org');
    const role = (ROLES as readonly string[]).includes(asText(body.role)) ? (body.role as Role) : held.role;
    const leaving = this.under(held.domain).filter((node) => !this.world.places.pathTo(node.id).some((row) => row.id === to));
    const arriving = this.under(to).filter((node) => !this.world.places.pathTo(node.id).some((row) => row.id === held.domain));
    const reach = (nodes: { kind: NodeRef['kind'] }[]) => ({ nodes: nodes.filter((node) => node.kind === 'project' || node.kind === 'topic').length, resources: 0, documents: nodes.filter((node) => node.kind === 'topic').length * 4 });
    const missing: MemberMovePlan['missing'] = [];
    for (const place of [held.domain, to]) {
      if (this.world.places.access(person, place).permissions.includes('members.manage')) continue;
      const row = this.world.places.rowOf(place);
      const owner = this.world.places.userOfName(this.world.places.metaOf(place).owner);
      missing.push({
        object: { kind: 'org', id: place, name: row?.name ?? null, owner },
        permissions: ['members.manage'],
        options: null,
        ask: [owner],
        ask_more: 0,
        override: false,
        closed: null,
      });
    }
    return {
      user: userRef(target.me),
      from: { id: from.id, kind: 'org', name: from.name, redacted: false },
      to: { id: destination.id, kind: 'org', name: destination.name, redacted: false },
      role,
      loses: reach(leaving),
      gains: reach(arriving),
      stays: { nodes: this.ownedBy(target.me.username, root), resources: (this.made.get(target.me.username) ?? []).length, roles: [], named: 0 },
      waiting: [],
      missing,
    };
  }

  private moveMember(person: Person, orgId: string, userId: string, body: Record<string, unknown>): Answer {
    const plan = this.movePlan(person, orgId, userId, body);
    if (plan.missing.length > 0) {
      throw new Refusal(403, 'authz.denied', 'members.manage is required', { required: 'members.manage', object: { kind: 'org', id: plan.to.id }, requestable: true, override: false, missing: plan.missing });
    }
    const target = this.personById(userId);
    const root = this.rootOf(orgId);
    const held = this.world.places.roles.get(root)?.get(target.me.username);
    if (held === undefined) throw new Refusal(404, 'not_found', 'that person is no member here');
    held.domain = plan.to.id;
    held.role = plan.role;
    held.revision += 1;
    const member: Member = {
      user: userRef(target.me),
      domain: plan.to,
      address: `${(plan.to.name ?? '').toLowerCase()}.${target.me.username}`,
      role: plan.role,
      is_owner: this.world.places.metaOf(root).owner === target.me.username,
      since: held.since,
      also: this.also.get(target.me.username) ?? 0,
      suspended: false,
      last_seen_at: null,
      second_step: target.me.kind === 'service' ? null : target.me.totp_enabled || target.me.email_code_second_step ? 'enabled' : 'missing',
      second_step_required: target.me.kind === 'service' ? null : false,
    };
    return { status: 200, body: member };
  }

  private holdings(person: Person, orgId: string, userId: string): Holdings {
    const target = this.personOf(person, userId);
    if (target.me.id !== person.me.id) this.world.places.need(person, orgId, 'members.manage');
    return {
      nodes: this.ownedBy(target.me.username, this.rootOf(orgId)),
      shared: this.sharedWith.get(target.me.username) ?? 0,
      // What the double's people own with others are topics.
      shared_by_kind: { orgs: 0, projects: 0, topics: this.sharedWith.get(target.me.username) ?? 0, threads: 0, documents: 0, resources: 0 },
      resources: this.made.get(target.me.username) ?? [],
      machine_registrations: [],
      entries: 1,
      pending: 0,
    };
  }

  /** What a person owns together with others, by username. */
  private sharedWith = new Map<string, number>();

  private removeMember(person: Person, orgId: string, userId: string, body: Record<string, unknown>): Answer {
    const target = this.personOf(person, userId);
    if (target.me.id !== person.me.id) this.world.places.need(person, orgId, 'members.manage');
    const root = this.rootOf(orgId);
    const owner = this.world.places.metaOf(root).owner;
    if (owner === target.me.username) throw new Refusal(409, 'owner.last', 'name another owner first');
    const members = this.world.places.roles.get(root);
    if (members?.delete(target.me.username) !== true) throw new Refusal(404, 'not_found', 'that person is no member here');
    // What they owned alone passes to the first owner of the org it lies in.
    for (const row of this.world.trees.get(root)?.nodes.values() ?? []) {
      if (this.world.places.metaOf(row.id).owner === target.me.username) this.world.places.metaOf(row.id).owner = owner;
    }
    const fates = typeof body.resources === 'object' && body.resources !== null ? (body.resources as Record<string, unknown>) : {};
    this.made.set(
      target.me.username,
      (this.made.get(target.me.username) ?? []).filter((resource) => fates[resource.object.id] !== 'remove'),
    );
    for (const here of this.suspended.values()) here.delete(target.me.username);
    return { status: 204 };
  }

  // ---------------------------------------------------------------- the settings of a place

  /** The rows people set, by place and key. */
  private rows = new Map<string, Map<string, { value: Json; locked: boolean; revision: number }>>();
  /** The secrets set at a place: their values are kept so that a test can see that they arrived, and no request gives them back. */
  private secretsHeld = new Map<string, Map<string, { value: string; rotatedAt: string }>>();

  private ref(rowId: string): NodeRef {
    const row = this.world.places.rowOf(rowId);
    return { id: rowId, kind: row?.kind ?? 'org', name: row?.name ?? null, redacted: false };
  }

  /** The keys a place has: those that can be set at its kind or at a kind below it. */
  private keysOf(kind: string): Key[] {
    const order = ['system', 'org', 'project', 'topic', 'thread'];
    const from = order.indexOf(kind);
    return REGISTRY.filter((entry) => entry.at.some((scope) => order.indexOf(scope) >= from));
  }

  private mayWrite(person: Person, nodeId: string, entry: Key): boolean {
    if (this.world.places.rowOf(nodeId)?.archived === true) return false;
    const kind = this.world.places.rowOf(nodeId)?.kind ?? 'org';
    const held = this.world.places.access(person, nodeId).permissions;
    return held.includes(entry.def.class === 'policy' ? 'settings.manage' : `${kind}.manage`);
  }

  /** Where a key's value in force at a place is set: the topmost locked row on the way (it may be the place's own, and then it is not above the place), and the row the value is read from. */
  private whereSet(nodeId: string, entry: Key) {
    const chain = this.world.places.pathTo(nodeId);
    const name = entry.def.key;
    const honours = entry.def.merge !== 'intersect' && entry.def.merge !== 'min';
    const locking = chain.find((row) => honours && this.rows.get(row.id)?.get(name)?.locked === true);
    const above = locking !== undefined && locking.id !== nodeId ? locking : undefined;
    const deepest = [...chain].reverse().find((row) => this.rows.get(row.id)?.has(name) === true);
    const from = above ?? deepest;
    return { locking, above, from, row: from === undefined ? undefined : this.rows.get(from.id)?.get(name) };
  }

  private resolve(person: Person, nodeId: string, entry: Key): ResolvedSetting {
    const name = entry.def.key;
    const own = this.rows.get(nodeId)?.get(name) ?? null;
    const { locking, above, from, row } = this.whereSet(nodeId, entry);
    const joined = resolvedParts(entry.def, [
      { source: { scope: 'default', node: null }, value: entry.def.default, locked: false, ignored: false },
      ...this.world.places.pathTo(nodeId).flatMap((place) => {
        const part = this.rows.get(place.id)?.get(name);
        if (part === undefined) return [];
        const readable = this.world.places.access(person, place.id).permissions.includes(`${place.kind}.view`);
        const ref = this.ref(place.id);
        return [{ source: { scope: place.kind, node: readable ? ref : { ...ref, name: null, redacted: true } }, value: part.value, locked: part.locked, ignored: false }];
      }),
    ]);
    const resolved: SettingWithParts = {
      key: name,
      value: entry.def.merge === 'replace' ? row?.value ?? entry.def.default : joined.value,
      parts: joined.parts,
      source: from === undefined ? { scope: 'default', node: null } : { scope: from.kind, node: this.ref(from.id) },
      own: own === null ? null : { value: own.value, locked: own.locked, revision: own.revision },
      locked_by: locking === undefined ? null : { scope: locking.kind, node: this.ref(locking.id) },
      writable: above === undefined && this.mayWrite(person, nodeId, entry),
    };
    return resolved;
  }

  /** What a key is at a place: its value in force, and the place whose lock holds it there (the place itself, if it locked it), for the parts that act on what is set: the start of a thread reads its defaults here. */
  valueAt(nodeId: string, key: string): { value: unknown; lockedBy: string | null } {
    const entry = this.entryOf(key);
    const { locking, row } = this.whereSet(nodeId, entry);
    return { value: row?.value ?? entry.def.default, lockedBy: locking?.id ?? null };
  }

  /** Whether the registry has the key. */
  knows(key: string): boolean {
    return REGISTRY.some((entry) => entry.def.key === key);
  }

  /** Writes a key at a place as the system does when somebody with the right did, for a test that steers a default and for the seeds of the team. */
  setValue(nodeId: string, key: string, value: Json, locked = false): void {
    const held = this.rows.get(nodeId) ?? new Map<string, { value: Json; locked: boolean; revision: number }>();
    held.set(key, { value, locked, revision: (held.get(key)?.revision ?? 0) + 1 });
    this.rows.set(nodeId, held);
  }

  /** Takes what was written at a place away again. */
  clearValue(nodeId: string, key: string): void {
    this.rows.get(nodeId)?.delete(key);
  }

  /** The lock above a place that decides a key there, where one does: a lock at the place itself is not above it. */
  private lockAbove(person: Person, nodeId: string, entry: Key): ResolvedSetting['locked_by'] {
    const lock = this.resolve(person, nodeId, entry).locked_by;
    return lock?.node?.id === nodeId ? null : (lock ?? null);
  }

  /** The first thing wrong with a value, as the registry would say it, or `null`. */
  private wrong(schema: Json, value: Json): string | null {
    const shape = schema as Record<string, Json>;
    switch (shape.type) {
      case 'boolean':
        return typeof value === 'boolean' ? null : 'must be true or false';
      case 'integer': {
        if (typeof value !== 'number' || !Number.isInteger(value)) return 'must be a whole number';
        if (typeof shape.minimum === 'number' && value < shape.minimum) return `must be at least ${shape.minimum}`;
        if (typeof shape.maximum === 'number' && value > shape.maximum) return `must be at most ${shape.maximum}`;
        return null;
      }
      case 'string': {
        if (typeof value !== 'string') return 'must be text';
        if (Array.isArray(shape.enum) && !shape.enum.includes(value)) return `must be one of ${shape.enum.join(', ')}`;
        if (typeof shape.maxLength === 'number' && Array.from(value).length > shape.maxLength) return `must be at most ${shape.maxLength} characters`;
        return null;
      }
      case 'array': {
        if (!Array.isArray(value)) return 'must be a list';
        for (const [at, item] of value.entries()) {
          const fault = this.wrong(shape.items, item);
          if (fault !== null) return `[${at}] ${fault}`;
          if (shape.uniqueItems === true && value.indexOf(item) !== at) return `[${at}] is in the list twice`;
        }
        return null;
      }
      case 'object': {
        if (typeof value !== 'object' || value === null || Array.isArray(value)) return 'must be an object';
        const held = value as Record<string, Json>;
        const properties = shape.properties as Record<string, Json> | undefined;
        if (properties !== undefined) {
          for (const required of (shape.required as string[] | undefined) ?? []) if (!(required in held)) return `${required} is required`;
          for (const [member, inner] of Object.entries(held)) {
            if (!(member in properties)) return `${member} is no member of this value`;
            const fault = this.wrong(properties[member], inner);
            if (fault !== null) return `${member} ${fault}`;
          }
          return null;
        }
        for (const [member, inner] of Object.entries(held)) {
          const fault = this.wrong(shape.additionalProperties, inner);
          if (fault !== null) return `${member} ${fault}`;
        }
        return null;
      }
      default:
        return null;
    }
  }

  private entryOf(name: string): Key {
    const entry = REGISTRY.find((candidate) => candidate.def.key === name);
    if (entry === undefined) this.invalid('key', `${name} is no setting`);
    return entry;
  }

  private writeSetting(person: Person, nodeId: string, name: string, body: Record<string, unknown>): ResolvedSetting {
    this.world.places.need(person, nodeId, `${this.world.places.rowOf(nodeId)?.kind ?? 'org'}.view`);
    const entry = this.entryOf(name);
    const kind = this.world.places.rowOf(nodeId)?.kind ?? 'org';
    if (!entry.at.includes(kind)) this.invalid('key', `${name} is not set at the scope ${kind}`);
    const lock = this.lockAbove(person, nodeId, entry);
    if (lock !== null) {
      throw new Refusal(409, 'access.locked', 'a lock above holds this', { by: lock.node });
    }
    if (!this.mayWrite(person, nodeId, entry)) {
      const required = entry.def.class === 'policy' ? 'settings.manage' : `${kind}.manage`;
      this.world.places.need(person, nodeId, required);
    }
    const own = this.rows.get(nodeId)?.get(name);
    if (typeof body.expected_revision === 'number' && (own?.revision ?? 0) !== body.expected_revision) {
      throw new Refusal(409, 'conflict.stale_revision', 'the setting was changed meanwhile', { current: this.resolve(person, nodeId, entry) });
    }
    const fault = this.wrong(entry.def.schema, body.value);
    if (fault !== null) this.invalid('value', fault);
    const locked = typeof body.locked === 'boolean' ? body.locked : (own?.locked ?? false);
    if (locked !== (own?.locked ?? false) && !this.world.places.access(person, nodeId).permissions.includes('settings.manage')) {
      this.world.places.need(person, nodeId, 'settings.manage');
    }
    const rows = this.rows.get(nodeId) ?? new Map<string, { value: Json; locked: boolean; revision: number }>();
    const same = own !== undefined && JSON.stringify(own.value) === JSON.stringify(body.value) && own.locked === locked;
    if (!same) rows.set(name, { value: body.value, locked, revision: (own?.revision ?? 0) + 1 });
    this.rows.set(nodeId, rows);
    return this.resolve(person, nodeId, entry);
  }

  private clearSetting(person: Person, nodeId: string, name: string, expected?: number): ResolvedSetting {
    const entry = this.entryOf(name);
    const lock = this.lockAbove(person, nodeId, entry);
    if (lock !== null) throw new Refusal(409, 'access.locked', 'a lock above holds this', { by: lock.node });
    if (!this.mayWrite(person, nodeId, entry)) this.world.places.need(person, nodeId, entry.def.class === 'policy' ? 'settings.manage' : `${this.world.places.rowOf(nodeId)?.kind ?? 'org'}.manage`);
    const own = this.rows.get(nodeId)?.get(name);
    if (expected !== undefined && own?.revision !== expected) {
      const kind = this.world.places.rowOf(nodeId)?.kind ?? 'org';
      const view = this.world.places.access(person, nodeId).permissions.includes(`${kind}.view`);
      throw new Refusal(409, 'conflict.stale_revision', 'the setting was changed meanwhile', view ? { current: this.resolve(person, nodeId, entry) } : {});
    }
    this.rows.get(nodeId)?.delete(name);
    return this.resolve(person, nodeId, entry);
  }

  private secretList(nodeId: string): SecretInfo[] {
    const found = new Map<string, SecretInfo>();
    // The nearest place that sets a name is the one that counts.
    for (const row of this.world.places.pathTo(nodeId)) {
      for (const [name, held] of this.secretsHeld.get(row.id) ?? []) found.set(name, { name, node: this.ref(row.id), rotated_at: held.rotatedAt });
    }
    return [...found.values()].sort((one, other) => one.name.localeCompare(other.name));
  }

  private writeSecret(person: Person, nodeId: string, name: string, body: Record<string, unknown>): SecretInfo {
    this.world.places.need(person, nodeId, 'settings.manage');
    if (!/^[A-Z_][A-Z0-9_]{0,63}$/.test(name)) this.invalid('name', 'must be capitals, digits and underscores that begin with no digit');
    const value = asText(body.value);
    if (value === '') this.invalid('value', 'must not be empty');
    const held = this.secretsHeld.get(nodeId) ?? new Map<string, { value: string; rotatedAt: string }>();
    held.set(name, { value, rotatedAt: new Date().toISOString() });
    this.secretsHeld.set(nodeId, held);
    return { name, node: this.ref(nodeId), rotated_at: held.get(name)?.rotatedAt ?? null };
  }

  /** The secrets that were written, with their values, for a test to see that they arrived. */
  secretsWritten(): Record<string, string> {
    const written: Record<string, string> = {};
    for (const [nodeId, held] of this.secretsHeld) for (const [name, secret] of held) written[`${nodeId}/${name}`] = secret.value;
    return written;
  }

  /** What the team starts with, for the settings: Acme sets a value that places below inherit, and locks the sandbox. */
  seedSettings(): void {
    this.rows.set(ACME.org, new Map([
      ['env.idle_stop_minutes', { value: 720, locked: false, revision: 1 }],
      ['sandbox.mode', { value: 'read_only', locked: true, revision: 1 }],
    ]));
    // The way the team works, decided once for Web and for everything in it: Codex on the team's key, with the model it is usually run on.
    this.setValue(ACME.web, 'agent.default_provider_id', RUNS_ON.openAiKey);
    this.setValue(ACME.web, 'agent.default_agent_id', RUNS_ON.codex);
    this.setValue(ACME.web, 'agent.model', USUAL_MODEL.name);
    this.setValue(ACME.web, 'agent.effort', USUAL_MODEL.effort);
    this.secretsHeld.set(ACME.org, new Map([['REGISTRY_TOKEN', { value: 'seeded', rotatedAt: '2026-10-10T08:00:00.000Z' }]]));
  }

  // ---------------------------------------------------------------- channels and destinations

  /** Every channel there is, by whose it is: an org's, or the system's (`SYSTEM_CHANNELS`). The ways a person is routed to them are the notifications double's. */
  private channels = new Map<string, NotificationChannel[]>();
  private destinations = new Map<string, Destination[]>();

  /** Every channel the person can route to: the system's, and those of the orgs they belong to. */
  channelsVisibleTo(person: Person): NotificationChannel[] {
    const orgs = new Set(this.world.orgsOf(person).map((org) => org.id));
    return [...this.channels].filter(([orgId]) => orgId === SYSTEM_CHANNELS || orgs.has(orgId)).flatMap(([, list]) => list);
  }

  /** Whether the person may change the channels of an org, or those of the system. */
  private mayManageChannels(person: Person, orgId: string): void {
    if (orgId === SYSTEM_CHANNELS) {
      if (!person.me.is_system_admin) throw new Refusal(403, 'authz.denied', 'the channels of the system are changed by its administrators', { required: 'system.manage', requestable: false, override: false, missing: [] });
      return;
    }
    this.world.places.need(person, orgId, 'settings.manage');
  }

  /** Refuses what a kind of channel would refuse: a required field left empty, and what the far side says for good. */
  private checkFields(kind: Notifier, config: Record<string, unknown>, secrets: Record<string, unknown>, setAlready: readonly string[] = []): void {
    const errors: { path: string; message: string }[] = [];
    for (const entry of kind.fields) {
      const secret = entry.type === 'secret' || entry.type === 'secret_text' || entry.type === 'secret_map';
      const given = secret ? secrets[entry.name] : config[entry.name];
      const empty = given === undefined || given === '' || given === null;
      if (entry.required && empty && !(secret && setAlready.includes(entry.name))) errors.push({ path: `${secret ? 'secrets' : 'config'}.${entry.name}`, message: 'is required' });
    }
    if (errors.length === 0 && kind.name === 'email' && config.host === 'smtp.refuses.example') {
      errors.push({ path: 'secrets.password', message: 'the mail server answered: 535 5.7.8 authentication failed' });
    }
    if (errors.length > 0) throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors });
  }

  private notifier(name: string): Notifier {
    const found = NOTIFIERS.find((entry) => entry.name === name);
    if (found === undefined) this.invalid('kind', `${name} is no kind of channel`);
    return found;
  }

  private channelOf(id: string): { channel: NotificationChannel; orgId: string } {
    for (const [orgId, list] of this.channels) {
      const channel = list.find((entry) => entry.id === id);
      if (channel !== undefined) return { channel, orgId };
    }
    throw new Refusal(404, 'not_found', 'no such channel');
  }

  private newChannel(orgId: string, kind: string, body: Record<string, unknown>): NotificationChannel {
    const row = this.world.places.rowOf(orgId);
    return {
      id: randomUUID(),
      kind,
      name: asText(body.name).trim(),
      owner: { org: { id: orgId, kind: 'org', name: row?.name ?? null, redacted: false } },
      config: typeof body.config === 'object' && body.config !== null ? body.config : {},
      secrets_set: Object.keys(typeof body.secrets === 'object' && body.secrets !== null ? body.secrets : {}),
      templates: {},
      enabled: body.enabled !== false,
      can_edit: true,
      enabled_route_count: 0,
      last_ok_at: null,
      last_error: null,
      route: {
        id: null,
        channel_id: '',
        channel: null,
        enabled: false,
        health: 'never',
        last_ok_at: null,
        failed_at: null,
        failure: null,
        waiting_count: 0,
        kinds: [],
        label: null,
        last_sent_at: null,
        link: null,
        min_interval_minutes: null,
        needs: null,
        unread_delay_minutes: null,
      },
    };
  }

  /** What a test of something reaching `url` says: the far side's own words where it fails. */
  private farSide(config: unknown): { ok: boolean; error: string | null } {
    const url = typeof config === 'object' && config !== null ? asText((config as Record<string, unknown>).url) : '';
    if (url.includes('failing')) return { ok: false, error: 'HTTP 502 Bad Gateway from hooks.example: upstream timed out after 10s' };
    return { ok: true, error: null };
  }

  private seedChannels(): void {
    const ops = this.newChannel(ACME.org, 'webhook', { name: 'Ops room', config: { url: 'https://hooks.example/ops' }, secrets: { secret: 'signing' } });
    const bot = this.newChannel(ACME.org, 'telegram', { name: "Acme's bot", secrets: { token: '1234:team' } });
    this.channels.set(ACME.org, [ops, bot]);
    // The system's own: mail and push, which every org may route to and none may change.
    const system = (kind: string, name: string): NotificationChannel => ({ ...this.newChannel(ACME.org, kind, { name }), owner: 'system' });
    this.channels.set(SYSTEM_CHANNELS, [system('email', 'System mail'), system('push', 'System push')]);
    const room: Destination = {
      id: randomUUID(),
      channel: 'webhook',
      name: 'Build room',
      node: this.ref(ACME.web),
      config: { url: 'https://hooks.example/failing/build' },
      secrets_set: [],
      templates: {},
      kinds: ['turn.failed'],
      level: 'important',
      enabled: true,
      last_ok_at: null,
      last_error: null,
      created_at: '2026-10-05T09:00:00.000Z',
    };
    this.destinations.set(ACME.web, [room]);
  }

  // ---------------------------------------------------------------- service identities and the audit trail

  private accounts = new Map<string, ServiceAccount[]>();
  private accountTokens = new Map<string, TokenHeld[]>();

  private accountOf(id: string): { account: ServiceAccount; orgId: string } {
    for (const [orgId, list] of this.accounts) {
      const account = list.find((entry) => entry.user.id === id);
      if (account !== undefined) return { account, orgId };
    }
    throw new Refusal(404, 'not_found', 'no such service account');
  }

  private seedAccounts(): void {
    const ada = this.world.people.get('ada');
    if (ada === undefined) return;
    const user: UserRef = { id: randomUUID(), kind: 'service', username: 'ci-bot', display_name: 'CI bot', avatar: { text: 'C', color: 6 } };
    this.accounts.set(ACME.org, [{ user, created_by: userRef(ada.me), created_at: '2026-10-04T10:00:00.000Z', token_count: 0 }]);
  }

  /** What was done, as the audit trail lists it: the history of the place, and one act that an admin forced. */
  private auditOf(nodeId: string): AuditEntry[] {
    const lines = this.world.places.lines.filter((line) => line.node === nodeId || line.parent === nodeId || this.world.places.pathTo(line.node).some((row) => row.id === nodeId));
    const entries: AuditEntry[] = lines.map((line, at) => ({
      id: `audit-${at}`,
      at: line.at,
      actor: { kind: 'user', user: this.world.places.userOfName(line.actor) },
      action: line.action,
      kind: line.action === 'node.created' ? 'made' : line.action === 'node.deleted' ? 'deleted' : 'changed',
      request_id: null,
      event_deletion_id: null,
      object: { kind: this.world.places.rowOf(line.node)?.kind ?? 'org', id: line.node, name: this.ref(line.node).name ?? null, state: 'live', deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: true, can_inspect: false, can_restore: false },
      node: this.ref(line.node),
      summary: line.title,
      details: { by: line.actor },
      history: true,
      as_admin: false,
      override: false,
      ip: '203.0.113.7',
    }));
    entries.unshift({
      id: 'audit-bypass',
      at: '2026-10-11T12:00:00.000Z',
      actor: { kind: 'user', user: this.world.places.userOfName('ada') },
      action: 'permission.added',
      kind: 'access',
      request_id: null,
      event_deletion_id: null,
      object: { kind: 'project', id: ACME.web, name: 'Web', state: 'live', deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: true, can_inspect: false, can_restore: false },
      node: this.ref(ACME.web),
      summary: 'Ada Lovelace named herself on the page of Web',
      details: { override: true },
      history: true,
      as_admin: false,
      override: true,
      ip: null,
    });
    return entries;
  }

  // ---------------------------------------------------------------- tokens

  private tokensOf(person: Person): TokenHeld[] {
    let list = this.tokens.get(person.me.username);
    if (list === undefined) {
      list = [];
      this.tokens.set(person.me.username, list);
    }
    return list;
  }

  private nodeRef(nodeId: string): NodeRef {
    const row = this.world.places.rowOf(nodeId);
    if (row === undefined) this.invalid('node_id', 'is no place');
    return { id: row.id, kind: row.kind, name: row.name, redacted: false };
  }

  private makeToken(person: Person, body: Record<string, unknown>): Answer {
    const name = asText(body.name).trim();
    if (name === '') this.invalid('name', 'must not be empty');
    const scopes = Array.isArray(body.scopes) ? body.scopes.filter((scope): scope is string => typeof scope === 'string') : ['*'];
    if (scopes.length === 0) this.invalid('scopes', 'must name at least one permission');
    const info: ApiTokenInfo = {
      id: randomUUID(),
      name,
      scopes,
      node: typeof body.node_id === 'string' ? this.nodeRef(body.node_id) : null,
      created_at: new Date().toISOString(),
      expires_at: typeof body.expires_at === 'string' ? body.expires_at : null,
      last_used_at: null,
    };
    const secret = `opat_${randomUUID().replaceAll('-', '')}`;
    this.tokensOf(person).push({ info, secret });
    return { status: 200, body: { token: secret, info } };
  }

  // ---------------------------------------------------------------- what the interface keeps

  /** The caller's one portable-state record, shared by parts which add a personal key. */
  stateOf(person: Person): Record<string, unknown> {
    let held = this.state.get(person.me.username);
    if (held === undefined) {
      held = {};
      this.state.set(person.me.username, held);
    }
    return held;
  }

  /** What the team starts with: Ada has a viewer chosen, a tutorial finished and hints closed; Dora's orgs allow a mailed code as a second step. */
  seedTeam(): void {
    this.state.set('ada', {
      viewers: { markdown: 'source' },
      help: { finished: ['first-thread'], closed: ['hint.binding', 'hint.queue'] },
    });
    this.seedSettings();
    this.setValue(ACME.fixLogin, 'env.default_id', RUNS_ON.workspace);
    this.seedChannels();
    this.seedAccounts();
    const dora = this.world.people.get('dora');
    if (dora !== undefined) dora.me = { ...dora.me, email_code: ['second_step'] };
    // Grace owns a topic with a thread in it, and holds a role by name in a project besides her place; Dora made two things others use.
    this.world.places.metaOf(ACME.upgradeDeps).owner = 'grace';
    this.world.places.metaOf(ACME.depsThread).owner = 'grace';
    this.also.set('grace', 1);
    this.sharedWith.set('grace', 2);
    if (dora !== undefined) {
      this.made.set('dora', [
        { object: { kind: 'provider', id: id(7, 1), name: 'Dora’s API key' }, shared_with: 2, threads: 1, personal_sign_in: false },
        { object: { kind: 'provider', id: id(7, 2), name: 'Dora’s Claude subscription' }, shared_with: 1, threads: 0, personal_sign_in: true },
      ]);
    }
  }

  // ---------------------------------------------------------------- what only a test can do

  /** The values of the secrets that were written, to see that they arrived (`/__double/settings/secrets`): no request of the API gives one back. */
  control(path: string, body: Record<string,unknown> = {}): Answer | null {
    if (path === '/account') {
      const person=this.world.people.get(typeof body.username === 'string' ? body.username : '');
      if (person === undefined) return {status:404};
      if (typeof body.has_password === 'boolean') this.setMe(person,{has_password:body.has_password});
      if (typeof body.email_code_second_step === 'boolean') this.setMe(person,{email_code_second_step:body.email_code_second_step});
      return {status:200,body:person.me};
    }
    return path === '/secrets' ? { status: 200, body: this.secretsWritten() } : null;
  }

  // ---------------------------------------------------------------- the requests

  /** Answers a request about settings, or `null` when it is not one. */
  handle(person: Person, method: string, path: string, _query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    switch (route) {
      case 'POST /me/password':
        return this.changePassword(person, body);
      case 'POST /me/totp/setup':
        return this.totpSetup(person, body);
      case 'POST /me/totp/enable':
        return this.totpEnable(person, body);
      case 'GET /me/recovery-codes':
        return { status: 200, body: this.recoveryStatus(person) };
      case 'POST /me/recovery-codes':
        return this.recoveryRegenerate(person, body);
      case 'POST /me/second-step/verify':
        return this.verifyStep(person, body);
      case 'POST /me/totp/disable':
        return this.totpDisable(person, body);
      case 'POST /me/email-code/send':
        return this.mailCodeSend(person, body);
      case 'POST /me/email-code/enable':
        return this.mailCodeEnable(person, body);
      case 'POST /me/email-code/disable':
        return this.mailCodeDisable(person, body);
      case 'GET /me/sessions':
        return { status: 200, body: this.sessionsOf(person) };
      case 'DELETE /me/sessions': {
        const kept = this.sessionsOf(person).filter((session) => session.current);
        this.world.revokeBrowserSessions(person);
        this.sessions.set(person.me.username, this.sessionTemplates(person).filter((session) => session.current));
        return { status: 200, body: kept };
      }
      case 'GET /me/tokens':
        return { status: 200, body: this.tokensOf(person).map((held) => held.info) };
      case 'POST /me/tokens':
        return this.makeToken(person, body);
      case 'GET /me/state':
        return { status: 200, body: this.stateOf(person) };
      default:
    }
    if ((found = /^GET \/nodes\/([^/]+)\/audit$/.exec(route))) {
      const nodeId = found[1] ?? '';
      this.world.places.need(person, nodeId, 'settings.manage');
      const action = _query.get('action') ?? '';
      const actor = _query.get('actor');
      const from = _query.get('from');
      const to = _query.get('to');
      const items = this.auditOf(nodeId).filter(
        (entry) =>
          (action === '' || entry.action === action) &&
          (actor === null || (entry.actor.kind === 'user' && entry.actor.user.id === actor)) &&
          (from === null || entry.at >= from) &&
          (to === null || entry.at <= to),
      );
      return { status: 200, body: { items, next_cursor: null } };
    }
    if ((found = /^GET \/orgs\/([^/]+)\/service-accounts$/.exec(route))) {
      this.world.places.need(person, found[1] ?? '', 'members.manage');
      return { status: 200, body: (this.accounts.get(found[1] ?? '') ?? []).map((entry) => ({ ...entry, token_count: (this.accountTokens.get(entry.user.id) ?? []).length })) };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/service-accounts$/.exec(route))) {
      const orgId = found[1] ?? '';
      this.world.places.need(person, orgId, 'members.manage');
      const username = asText(body.username);
      if (!/^[a-z0-9][a-z0-9_-]{2,31}$/.test(username)) this.invalid('username', 'must be 3 to 32 of a-z, 0-9, - and _');
      if ([...this.world.people.keys()].includes(username) || [...this.accounts.values()].flat().some((entry) => entry.user.username === username)) throw new Refusal(409, 'conflict.exists', 'the name is taken');
      const made: ServiceAccount = {
        user: { id: randomUUID(), kind: 'service', username, display_name: asText(body.display_name).trim(), avatar: { text: asText(body.display_name).trim().slice(0, 1).toUpperCase(), color: 3 } },
        created_by: userRef(person.me),
        created_at: new Date().toISOString(),
        token_count: 0,
      };
      this.accounts.set(orgId, [...(this.accounts.get(orgId) ?? []), made]);
      return { status: 200, body: made };
    }
    if ((found = /^DELETE \/service-accounts\/([^/]+)$/.exec(route))) {
      const { account, orgId } = this.accountOf(found[1] ?? '');
      this.world.places.need(person, orgId, 'members.manage');
      this.accounts.set(orgId, (this.accounts.get(orgId) ?? []).filter((entry) => entry !== account));
      this.accountTokens.delete(account.user.id);
      return { status: 204 };
    }
    if ((found = /^(GET|POST) \/service-accounts\/([^/]+)\/tokens$/.exec(route))) {
      const { account, orgId } = this.accountOf(found[2] ?? '');
      this.world.places.need(person, orgId, 'members.manage');
      const held = this.accountTokens.get(account.user.id) ?? [];
      if (found[1] === 'GET') return { status: 200, body: held.map((entry) => entry.info) };
      const name = asText(body.name).trim();
      if (name === '') this.invalid('name', 'must not be empty');
      const info: ApiTokenInfo = { id: randomUUID(), name, scopes: ['*'], node: null, created_at: new Date().toISOString(), expires_at: null, last_used_at: null };
      const secret = `opat_${randomUUID().replaceAll('-', '')}`;
      this.accountTokens.set(account.user.id, [...held, { info, secret }]);
      return { status: 200, body: { token: secret, info } };
    }
    if ((found = /^DELETE \/service-accounts\/([^/]+)\/tokens\/([^/]+)$/.exec(route))) {
      const { account, orgId } = this.accountOf(found[1] ?? '');
      this.world.places.need(person, orgId, 'members.manage');
      this.accountTokens.set(account.user.id, (this.accountTokens.get(account.user.id) ?? []).filter((entry) => entry.info.id !== found?.[2]));
      return { status: 204 };
    }
    if (route === 'GET /notifiers') return { status: 200, body: NOTIFIERS };
    if (route === 'GET /notification-channels') {
      // The channels of an org, for whoever sets them up; what a person can route to, without an org, is the notifications double's to say.
      const orgId = _query.get('org_id');
      if (orgId === null) return null;
      this.mayManageChannels(person, orgId);
      return { status: 200, body: this.channels.get(orgId) ?? [] };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/notification-channels$/.exec(route))) {
      const orgId = found[1] ?? '';
      this.world.places.need(person, orgId, 'settings.manage');
      const kind = this.notifier(asText(body.kind));
      const list = this.channels.get(orgId) ?? [];
      if (kind.single && list.some((entry) => entry.kind === kind.name)) throw new Refusal(409, 'conflict.exists', 'the org has one already');
      if (asText(body.name).trim() === '') this.invalid('name', 'must not be empty');
      this.checkFields(kind, (body.config ?? {}) as Record<string, unknown>, (body.secrets ?? {}) as Record<string, unknown>);
      const made = this.newChannel(orgId, kind.name, body);
      this.channels.set(orgId, [...list, made]);
      return { status: 200, body: made };
    }
    if ((found = /^PATCH \/notification-channels\/([^/]+)$/.exec(route))) {
      const { channel, orgId } = this.channelOf(found[1] ?? '');
      this.mayManageChannels(person, orgId);
      const kind = this.notifier(channel.kind);
      if (typeof body.config === 'object' && body.config !== null) {
        const secretsGiven = (body.secrets ?? {}) as Record<string, unknown>;
        this.checkFields(kind, body.config as Record<string, unknown>, secretsGiven, channel.secrets_set);
        channel.config = body.config;
        channel.secrets_set = [...new Set([...channel.secrets_set, ...Object.keys(secretsGiven)])];
      }
      if (typeof body.name === 'string' && body.name.trim() !== '') channel.name = body.name.trim();
      if (typeof body.enabled === 'boolean') channel.enabled = body.enabled;
      return { status: 200, body: channel };
    }
    if ((found = /^DELETE \/notification-channels\/([^/]+)$/.exec(route))) {
      const { channel, orgId } = this.channelOf(found[1] ?? '');
      this.mayManageChannels(person, orgId);
      this.channels.set(orgId, (this.channels.get(orgId) ?? []).filter((entry) => entry !== channel));
      return { status: 204 };
    }
    if ((found = /^POST \/notification-channels\/([^/]+)\/test$/.exec(route))) {
      const { channel } = this.channelOf(found[1] ?? '');
      if (channel.kind === 'email' && asText(body.to) === '') this.invalid('to', 'is needed to try a mail server');
      const answer = this.farSide(channel.config);
      if (answer.ok) {
        channel.last_ok_at = new Date().toISOString();
        channel.last_error = null;
      } else channel.last_error = answer.error;
      return { status: 200, body: answer };
    }
    if ((found = /^GET \/nodes\/([^/]+)\/destinations$/.exec(route))) {
      this.world.places.node(person, found[1] ?? '');
      return { status: 200, body: this.destinations.get(found[1] ?? '') ?? [] };
    }
    if ((found = /^POST \/nodes\/([^/]+)\/destinations$/.exec(route))) {
      const nodeId = found[1] ?? '';
      this.world.places.need(person, nodeId, 'settings.manage');
      const kind = this.notifier(asText(body.channel) === '' ? 'webhook' : asText(body.channel));
      if (!kind.targets.includes('destination')) this.invalid('channel', `${kind.name} cannot be a destination`);
      if (asText(body.name).trim() === '') this.invalid('name', 'must not be empty');
      this.checkFields(kind, (body.config ?? {}) as Record<string, unknown>, (body.secrets ?? {}) as Record<string, unknown>);
      const made: Destination = {
        id: randomUUID(),
        channel: kind.name,
        name: asText(body.name).trim(),
        node: this.ref(nodeId),
        config: body.config ?? {},
        secrets_set: Object.keys(body.secrets ?? {}),
        templates: {},
        kinds: Array.isArray(body.kinds) ? body.kinds.filter((entry): entry is string => typeof entry === 'string') : [],
        level: body.level === 'all' ? 'all' : 'important',
        enabled: body.enabled !== false,
        last_ok_at: null,
        last_error: null,
        created_at: new Date().toISOString(),
      };
      this.destinations.set(nodeId, [...(this.destinations.get(nodeId) ?? []), made]);
      return { status: 200, body: made };
    }
    if ((found = /^(PATCH|DELETE|POST) \/destinations\/([^/]+?)(\/test)?$/.exec(route))) {
      const id = found[2] ?? '';
      for (const [nodeId, list] of this.destinations) {
        const destination = list.find((entry) => entry.id === id);
        if (destination === undefined) continue;
        this.world.places.need(person, nodeId, 'settings.manage');
        if (found[1] === 'DELETE') {
          this.destinations.set(nodeId, list.filter((entry) => entry !== destination));
          return { status: 204 };
        }
        if (found[3] === '/test') {
          const answer = this.farSide(destination.config);
          if (answer.ok) {
            destination.last_ok_at = new Date().toISOString();
            destination.last_error = null;
          } else destination.last_error = answer.error;
          return { status: 200, body: answer };
        }
        const kind = this.notifier(destination.channel);
        if (typeof body.config === 'object' && body.config !== null) {
          this.checkFields(kind, body.config as Record<string, unknown>, (body.secrets ?? {}) as Record<string, unknown>, destination.secrets_set);
          destination.config = body.config;
        }
        if (typeof body.name === 'string' && body.name.trim() !== '') destination.name = body.name.trim();
        if (typeof body.enabled === 'boolean') destination.enabled = body.enabled;
        if (Array.isArray(body.kinds)) destination.kinds = body.kinds.filter((entry): entry is string => typeof entry === 'string');
        if (body.level === 'all' || body.level === 'important') destination.level = body.level;
        return { status: 200, body: destination };
      }
      throw new Refusal(404, 'not_found', 'no such destination');
    }
    if (route === 'GET /settings/registry') return { status: 200, body: REGISTRY.map((entry) => entry.def) };
    if ((found = /^GET \/nodes\/([^/]+)\/settings$/.exec(route))) {
      const nodeId = found[1] ?? '';
      this.world.places.node(person, nodeId);
      const kind = this.world.places.rowOf(nodeId)?.kind ?? 'org';
      const rows = [...this.keysOf(kind).map((entry) => this.resolve(person, nodeId, entry)), ...(this.world.access.sharingRows(nodeId) as ResolvedSetting[]).map((row) => ({ ...row, writable: row.writable && this.world.places.access(person, nodeId).permissions.includes('settings.manage') }))];
      if (kind !== 'thread') return { status: 200, body: rows };
      const choices = this.world.threads.settingsOf(nodeId).map((row) => ({ ...row, writable: row.writable && this.world.places.access(person, nodeId).permissions.includes('thread.manage') }));
      return { status: 200, body: [...rows.filter((row) => !choices.some((choice) => choice.key === row.key)), ...choices] };
    }
    if ((found = /^PUT \/nodes\/([^/]+)\/settings\/([^/]+)$/.exec(route))) return { status: 200, body: this.writeSetting(person, found[1] ?? '', found[2] ?? '', body) };
    if ((found = /^DELETE \/nodes\/([^/]+)\/settings\/([^/]+)$/.exec(route))) {
      const raw = _query.get('expected_revision');
      if (raw !== null && (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw)))) this.invalid('expected_revision', 'is a revision');
      return { status: 200, body: this.clearSetting(person, found[1] ?? '', found[2] ?? '', raw === null ? undefined : Number(raw)) };
    }
    if ((found = /^GET \/nodes\/([^/]+)\/secrets$/.exec(route))) {
      this.world.places.node(person, found[1] ?? '');
      return { status: 200, body: this.secretList(found[1] ?? '') };
    }
    if ((found = /^PUT \/nodes\/([^/]+)\/secrets\/([^/]+)$/.exec(route))) return { status: 200, body: this.writeSecret(person, found[1] ?? '', found[2] ?? '', body) };
    if ((found = /^DELETE \/nodes\/([^/]+)\/secrets\/([^/]+)$/.exec(route))) {
      this.world.places.need(person, found[1] ?? '', 'settings.manage');
      this.secretsHeld.get(found[1] ?? '')?.delete(found[2] ?? '');
      return { status: 204 };
    }
    if ((found = /^GET \/orgs\/([^/]+)\/members$/.exec(route))) return this.memberList(person, found[1] ?? '', _query);
    // Nobody from outside reaches anything of the fake world's orgs, and no token is listed for one: both answer empty pages to whoever manages its members.
    if ((found = /^GET \/orgs\/([^/]+)\/(outsiders|tokens)$/.exec(route))) {
      this.world.places.need(person, found[1] ?? '', 'members.manage');
      return { status: 200, body: { items: [], next_cursor: null } };
    }
    if ((found = /^GET \/orgs\/([^/]+)\/suspensions$/.exec(route))) {
      const orgId = found[1] ?? '';
      this.world.places.need(person, orgId, 'members.manage');
      return { status: 200, body: [...(this.suspended.get(orgId)?.values() ?? [])].map((held) => held.suspension) };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/members\/([^/]+)\/suspend$/.exec(route))) return this.suspend(person, found[1] ?? '', found[2] ?? '', body);
    if ((found = /^POST \/orgs\/([^/]+)\/members\/([^/]+)\/resume$/.exec(route))) {
      this.world.places.need(person, found[1] ?? '', 'members.manage');
      this.suspended.get(found[1] ?? '')?.delete(this.personById(found[2] ?? '').me.username);
      return { status: 204 };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/members\/([^/]+)\/move\/preview$/.exec(route))) return { status: 200, body: this.movePlan(person, found[1] ?? '', found[2] ?? '', body) };
    if ((found = /^POST \/orgs\/([^/]+)\/members\/([^/]+)\/move$/.exec(route))) return this.moveMember(person, found[1] ?? '', found[2] ?? '', body);
    if ((found = /^GET \/orgs\/([^/]+)\/members\/([^/]+)\/holdings$/.exec(route))) return { status: 200, body: this.holdings(person, found[1] ?? '', found[2] ?? '') };
    if ((found = /^DELETE \/orgs\/([^/]+)\/members\/([^/]+)$/.exec(route))) return this.removeMember(person, found[1] ?? '', found[2] ?? '', body);
    if ((found = /^DELETE \/me\/sessions\/([^/]+)$/.exec(route))) {
      const sessions = this.sessionsOf(person);
      const session = sessions.find((entry) => entry.id === found?.[1]);
      if (session === undefined) throw new Refusal(404, 'not_found', 'no such session');
      if (session.current) throw new Refusal(409, 'conflict.exists', 'sign out of this browser from the menu of your account');
      this.world.revokeBrowserSessions(person, session.id);
      this.sessions.set(person.me.username, this.sessionTemplates(person).filter((entry) => entry.id !== session.id));
      return { status: 204 };
    }
    if ((found = /^DELETE \/me\/tokens\/([^/]+)$/.exec(route))) {
      const list = this.tokensOf(person);
      if (!list.some((held) => held.info.id === found?.[1])) throw new Refusal(404, 'not_found', 'no such token');
      this.tokens.set(person.me.username, list.filter((held) => held.info.id !== found?.[1]));
      return { status: 204 };
    }
    if ((found = /^PUT \/me\/state\/([^/]+)$/.exec(route))) {
      const key = found[1] ?? '';
      if (!(STATE_KEYS as readonly string[]).includes(key)) this.invalid('key', 'is none of those the interface keeps');
      const fault = key === 'keys' ? keysFault(body) : null;
      if (fault !== null) this.invalid(fault.split(':')[0] ?? 'keys', fault.split(': ')[1] ?? fault);
      // The value is the whole body: an object, or what a list or a text is written as.
      this.stateOf(person)[key] = body;
      this.world.users.tell(person.me.username, { what: 'state', key, value: body });
      return { status: 204 };
    }
    return null;
  }
}

/** A code as the real API mails them: letters and digits that are not mistaken for one another. */
function randomCode(): string {
  const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  let code = '';
  for (let at = 0; at < 4; at += 1) code += alphabet[Math.floor(Math.random() * alphabet.length)] ?? '0';
  return code;
}
