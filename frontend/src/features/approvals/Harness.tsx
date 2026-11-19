/** Command, file, permission, question and elicitation decisions.
 *
 * What the agent's harness asks before it acts, in full, and the ways to answer it. A command is
 * shown as a code block with the directory it would run in; changes to files as differences; the
 * permissions asked for as a tree; questions as a form (a choice among the options, or words of the
 * person's own); and what an MCP server asks of the person as a form made from the shape it wants
 * back. One button for each option the harness offers, in the order it gave them: an option that
 * allows is a secondary button, one that denies is the danger kind, and none of them has the focus
 * when the card appears.
 */
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type {
  Approval,
  ApprovalDecision,
  ApprovalMatcher,
  ApprovalOption,
  ElicitationAction,
  HarnessRequest,
  Question,
} from '@/generated/api-types';
import { FileChanges } from '@/features/timeline/tools/FileChange';
import { useApprovalSubject } from '@/features/approvals/Card';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { CodeBlock } from '@/ui/CodeBlock';
import { Markdown } from '@/ui/Markdown';
import { Input } from '@/ui/Input';
import { JsonTree } from '@/ui/JsonTree';
import { RadioGroup } from '@/ui/RadioGroup';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Textarea } from '@/ui/Textarea';

export type HarnessApproval = Extract<Approval, { kind: 'harness' }>;

/** The request, in full. */
export function RequestBody({ request }: { request: HarnessRequest }) {
  const { t } = useTranslation();
  switch (request.request_kind) {
    case 'command':
      return (
        <div className="flex flex-col gap-2">
          <CodeBlock code={request.detail.command} lang="sh" />
          <p className="text-sm text-text-muted">{t('approvals.in', { directory: request.detail.cwd })}</p>
          {request.detail.reason !== null && request.detail.reason !== '' && <p className="text-sm text-text-muted">{request.detail.reason}</p>}
        </div>
      );
    case 'file_change':
      return (
        <div className="flex flex-col gap-2">
          <FileChanges changes={request.detail.changes} />
          {request.detail.reason !== null && request.detail.reason !== '' && <p className="text-sm text-text-muted">{request.detail.reason}</p>}
        </div>
      );
    case 'permissions':
      return (
        <div className="flex flex-col gap-2">
          {request.detail.reason !== null && request.detail.reason !== '' && <p className="text-sm text-text-muted">{request.detail.reason}</p>}
          <JsonTree value={request.detail.permissions} label={t('approvals.permissions')} />
        </div>
      );
    case 'other':
      return (
        <div className="flex flex-col gap-2">
          <p className="text-sm text-text">{request.detail.summary}</p>
          <JsonTree value={request.detail.raw} label={t('approvals.raw')} />
        </div>
      );
    case 'plan':
      return <div className="max-h-(--diff-max-height) overflow-y-auto text-sm" data-plan-text><Markdown text={request.detail.plan} /></div>;
    case 'user_input':
    case 'mcp_elicitation':
      return null;
  }
}

/** A plan's decision: proceeding allows leaving plan mode; keeping planning refuses it with the person's words. */
export function planDecision(proceed: boolean, words?: string): ApprovalDecision {
  return proceed ? { option_id: 'proceed' } : { option_id: 'keep_planning', ...(words === undefined ? {} : { note: words }) };
}

// ---------------------------------------------------------------- questions

function QuestionField({ question, value, other, onValue, onOther, disabled }: { question: Question; value: string[]; other: string; onValue: (value: string[]) => void; onOther: (text: string) => void; disabled: boolean }) {
  const { t } = useTranslation();
  const choices = question.options;
  const subject = useApprovalSubject();
  const inHeading = subject?.trim() === question.question.trim();
  const showHeader = question.header !== question.question && question.header.trim() !== '';
  return (
    <fieldset disabled={disabled} className="flex flex-col gap-2">
      <legend className={inHeading && !showHeader ? 'sr-only' : 'flex flex-col pb-1'}>
        {showHeader && <span className="text-xs font-medium text-text-subtle">{question.header}</span>}
        <span className={inHeading ? 'sr-only' : 'text-base text-text'}>{question.question}</span>
      </legend>
      {choices === null ? (
        <Textarea label={question.question} hideLabel rows={2} className="resize-none" value={value[0] ?? ''} onChange={(event) => onValue([event.target.value])} />
      ) : question.multiple ? (
        <div className="flex flex-col gap-1">
          {choices.map((choice) => (
            <Checkbox
              key={choice.label}
              label={choice.label}
              description={choice.description}
              checked={value.includes(choice.label)}
              onCheckedChange={(on) => onValue(on ? [...value, choice.label] : value.filter((entry) => entry !== choice.label))}
            />
          ))}
        </div>
      ) : (
        <RadioGroup
          variant="list"
          label={question.question}
          value={value[0] ?? ''}
          onValueChange={(next) => onValue([next])}
          options={choices.map((choice) => ({ value: choice.label, name: choice.label, description: choice.description }))}
        />
      )}
      {choices !== null && question.allow_other && (
        <Input label={t('approvals.other')} value={other} onChange={(event) => onOther(event.target.value)} />
      )}
    </fieldset>
  );
}

/** The questions as a form; the answers are sent when every question has one. */
export function AnswersForm({ questions, disabled, pending, onSend }: { questions: readonly Question[]; disabled: boolean; pending: boolean; onSend: (answers: Record<string, string[]>) => void }) {
  const { t } = useTranslation();
  const [values, setValues] = useState<Record<string, string[]>>({});
  const [others, setOthers] = useState<Record<string, string>>({});
  const answerOf = (question: Question): string[] => {
    const chosen = (values[question.id] ?? []).filter((entry) => entry.trim() !== '');
    const other = (others[question.id] ?? '').trim();
    return other === '' ? chosen : [...chosen, other];
  };
  const complete = questions.every((question) => answerOf(question).length > 0);
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (complete && !disabled) onSend(Object.fromEntries(questions.map((question) => [question.id, answerOf(question)])));
      }}
    >
      {questions.map((question) => (
        <QuestionField
          key={question.id}
          disabled={disabled}
          question={question}
          value={values[question.id] ?? []}
          other={others[question.id] ?? ''}
          onValue={(next) => setValues((all) => ({ ...all, [question.id]: next }))}
          onOther={(text) => setOthers((all) => ({ ...all, [question.id]: text }))}
        />
      ))}
      <div className="flex items-center gap-2">
        <Button type="submit" variant="secondary" pending={pending} disabled={disabled || !complete}>
          {t('approvals.sendAnswers')}
        </Button>
        {!complete && <span className="text-sm text-text-muted">{t('approvals.answerAll')}</span>}
      </div>
    </form>
  );
}

// ---------------------------------------------------------------- what an MCP server asks

interface SchemaProperty {
  type?: string;
  title?: string;
  description?: string;
  enum?: unknown[];
  default?: unknown;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;

/** A value of a schema's default or of its choices, as text; anything that is not a plain value reads as nothing. */
const plain = (value: unknown): string => (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean' ? String(value) : '');

/** The form for the shape an MCP server wants back, with the ways to answer it: accept, decline, cancel. */
export function ElicitationForm({ schema, message, disabled, pending, onSend }: { schema: unknown; message: string; disabled: boolean; pending: boolean; onSend: (action: ElicitationAction, content?: Record<string, unknown>) => void }) {
  const { t } = useTranslation();
  const inHeading = useApprovalSubject()?.trim() === message.trim();
  const properties = isRecord(schema) && isRecord(schema.properties) ? (schema.properties as Record<string, SchemaProperty>) : {};
  const required = isRecord(schema) && Array.isArray(schema.required) ? (schema.required as string[]) : [];
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(Object.entries(properties).map(([name, property]) => [name, property.default === undefined ? (property.type === 'boolean' ? 'false' : '') : plain(property.default)])),
  );
  const missing = required.some((name) => (values[name] ?? '') === '');
  const content = (): Record<string, unknown> =>
    Object.fromEntries(
      Object.entries(properties)
        .filter(([name]) => (values[name] ?? '') !== '' || properties[name]?.type === 'boolean')
        .map(([name, property]) => {
          const raw = values[name] ?? '';
          return [name, property.type === 'number' || property.type === 'integer' ? Number(raw) : property.type === 'boolean' ? raw === 'true' : raw];
        }),
    );
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (!missing && !disabled) onSend('accept', content());
      }}
    >
      {!inHeading && <p className="text-base text-text">{message}</p>}
      {Object.entries(properties).map(([name, property]) => {
        const label = property.title ?? name;
        const value = values[name] ?? '';
        const set = (next: string) => setValues((all) => ({ ...all, [name]: next }));
        if (property.type === 'boolean') return <Checkbox key={name} label={label} checked={value === 'true'} onCheckedChange={(on) => set(on ? 'true' : 'false')} />;
        if (Array.isArray(property.enum)) {
          return (
            <ChoiceLocator
              key={name}
              label={label}
              hint={property.description}
              value={value}
              valueLabel={value === '' ? plain(property.enum[0]) : undefined}
              disabled={disabled || pending}
              onValueChange={set}
              options={property.enum.map((option) => ({ value: plain(option), name: plain(option), description: property.description }))}
              optional={!required.includes(name)}
            />
          );
        }
        return (
          <Input
            key={name}
            label={label}
            hint={property.description}
            type={property.type === 'number' || property.type === 'integer' ? 'number' : 'text'}
            value={value}
            optional={!required.includes(name)}
            onChange={(event) => set(event.target.value)}
          />
        );
      })}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" variant="secondary" pending={pending} disabled={disabled || missing}>
          {t('approvals.accept')}
        </Button>
        <Button variant="ghost" disabled={disabled || pending} onClick={() => onSend('decline')}>
          {t('approvals.decline')}
        </Button>
        <Button variant="ghost" disabled={disabled || pending} onClick={() => onSend('cancel')}>
          {t('approvals.cancel')}
        </Button>
      </div>
    </form>
  );
}

// ---------------------------------------------------------------- the options

/**
 * What an option says, in the reader's language where it is one every harness has (allow once, allow for
 * the session, deny) and in the harness's own words where it is not (an allowance for one command's
 * prefix has a label that says which).
 */
export function optionWords(option: ApprovalOption, t: TFunction): string {
  if (option.style === 'allow' && option.scope === 'once') return t('approvals.allowOnce');
  if (option.style === 'allow' && option.scope === 'session') return t('approvals.allowSession');
  if (option.style === 'deny' && (option.scope === null || option.scope === 'once')) return t('approvals.deny');
  if (option.style === 'allow' && option.scope === 'thread' && option.rule !== null) return t('approvals.allowThread');
  return option.label;
}

/** What a rule allowed for a thread answers, in a line: the command it begins with, the command, or the host. */
export function matcherWords(matcher: ApprovalMatcher, t: TFunction): string {
  switch (matcher.type) {
    case 'command_prefix':
      return t('approvals.matcher.prefix', { command: matcher.argv.join(' '), cwd: matcher.cwd });
    case 'command_exact':
      return t('approvals.matcher.exact', { command: matcher.command, cwd: matcher.cwd });
    case 'host':
      return t('approvals.matcher.host', { host: matcher.host, protocol: matcher.protocol, port: matcher.port });
  }
}

/** One button for each option the harness offers, in the order it gave them. */
export function OptionButtons({ options, disabled, pressed, onChoose, target }: { target?: string; options: readonly ApprovalOption[]; disabled: boolean; pressed: string | null; onChoose: (option: ApprovalOption) => void }) {
  const { t } = useTranslation();
  // An option for this thread is offered only with the matcher it keeps, shown before it is chosen; one without
  // a matcher offers nothing for the thread.
  const offered = options.filter((option) => option.scope !== 'thread' || option.rule !== null);
  const rules = offered.flatMap((option) => (option.scope === 'thread' && option.rule !== null ? [option.rule] : []));
  return (
    <div className="flex flex-col gap-2">
      {rules.map((rule) => (
        <p key={matcherWords(rule, t)} className="font-mono text-xs wrap-anywhere text-text-muted" data-approval-rule>
          {matcherWords(rule, t)}
        </p>
      ))}
    <div className="flex flex-wrap items-center gap-2">
      {offered.map((option) => (
        <Button
          key={option.id}
          action={option.style === 'deny' ? 'deny' : 'allow'}
          aria-label={target === undefined ? optionWords(option, t) : `${optionWords(option, t)} · ${target}`}
          title={target === undefined ? optionWords(option, t) : `${optionWords(option, t)} · ${target}`}
          variant={option.style === 'deny' ? 'danger-ghost' : 'secondary'}
          size="md"
          pending={pressed === option.label}
          disabled={disabled}
          onClick={() => onChoose(option)}
        >
          {optionWords(option, t)}
        </Button>
      ))}
    </div>
    </div>
  );
}

/** The decision that an option stands for. */
export function decisionOfOption(option: ApprovalOption): ApprovalDecision {
  return { option_id: option.id };
}
