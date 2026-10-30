/**
 * Questions to people.
 *
 * dsh's `ask_user_question` tool, and its plan review, ask `ctx.userQuestions`, which hands the question to the first
 * listener of the `user-questions/request` waterfall that answers. dsh's ACP server has none, so the call fails with
 * "no provider". This extra answers it: it sends the questions to Orch as a request and waits as long as the person
 * takes, and turns the plan review into `_orch/plan/review`.
 */
import { randomUUID } from 'node:crypto'
import { RpcFault } from '../channel.ts'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import {
  ErrorCode,
  Method,
  type PlanDecision,
  type PlanReviewResult,
  type Question,
  type QuestionAnswer,
  type QuestionAskResult,
} from '../protocol.ts'
import { sessionIdOf } from './shared.ts'
import { toolCallBook } from './tool-calls.ts'

/** The request dsh puts to the waterfall. */
interface AskRequest {
  questions: Array<{
    id: string
    question: string
    detail?: string
    header?: string
    options?: Array<{ label: string; description?: string }>
    multiSelect?: boolean
    intent?: { kind: string; approve?: string; callId?: string }
  }>
  agent?: { id: string; header?: { id: string } }
  signal?: AbortSignal
  wait?: { callId: string; timed?: boolean }
}

interface Answer {
  answers: QuestionAnswer[]
}

/** Makes an error of dsh's own question class, so that its tools treat it as dsh does; a plain error if it cannot. */
async function questionError(host: ExtraHost, code: string, message: string): Promise<Error> {
  const service = host.service<{ ask(request: unknown): Promise<unknown> }>('userQuestions')
  let ErrorClass: (new (message: string, code: string) => Error) | undefined
  try {
    await service?.ask({ questions: [] })
  } catch (error) {
    const ctor = (error as { constructor?: unknown } | undefined)?.constructor
    if (typeof ctor === 'function' && (error as { name?: string }).name === 'UserQuestionError') {
      ErrorClass = ctor as new (message: string, code: string) => Error
    }
  }
  const made = ErrorClass !== undefined ? new ErrorClass(message, code) : Object.assign(new Error(message), { name: 'UserQuestionError', code })
  return made
}

function questionOf(item: AskRequest['questions'][number]): Question {
  const question: Question = { id: item.id, question: item.question }
  if (item.detail !== undefined) question.detail = item.detail
  if (item.header !== undefined) question.header = item.header
  if (item.options !== undefined) question.options = item.options.map((o) => (o.description === undefined ? { label: o.label } : { label: o.label, description: o.description }))
  if (item.multiSelect === true) question.multiSelect = true
  return question
}

export const questions: Extra = {
  id: 'questions',
  async start(host: ExtraHost) {
    if ((await host.waitService('userQuestions')) === undefined) throw new ExtraOff('host_api_missing', 'the user-questions service is not loaded')
    // The profile of dsh's ACP server does not load the tool a model asks questions with; its row has to be in the patch.
    const tools = host.service<{ get(name: string): unknown }>('tools')
    const deadline = Date.now() + host.config.serviceWaitMs
    while (tools?.get('ask_user_question') === undefined) {
      if (Date.now() > deadline) throw new ExtraOff('host_api_missing', 'the ask_user_question tool is not loaded: add the row of @deepseek-ai/dsh-tool-ask-user to the profile')
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    const calls = toolCallBook(host)
    const answer = async (request: AskRequest, next: () => Promise<Answer>): Promise<Answer> => {
      if (!host.isOn('questions') || host.channel.isClosed) return next()
      const sessionId = sessionIdOf(request.agent)
      if (sessionId === undefined || request.questions.length === 0) return next()
      const review = request.questions.find((q) => q.intent?.kind === 'plan-review')
      const callId = request.wait?.callId ?? review?.intent?.callId ?? calls.running(sessionId, 'ask_user_question') ?? randomUUID()
      const closed = (reason: 'cancelled' | 'turn_ended' | 'session_closed'): void =>
        host.channel.notify(Method.questionClosed, { sessionId, callId, reason })
      const withdraw = new AbortController()
      const onAbort = (): void => {
        closed('cancelled')
        withdraw.abort()
      }
      request.signal?.addEventListener('abort', onAbort, { once: true })
      try {
        if (review !== undefined) return await planReview(host, request, review, sessionId, callId, withdraw.signal)
        const result = await host.channel.request<QuestionAskResult>(
          Method.questionAsk,
          { sessionId, callId, questions: request.questions.map(questionOf) },
          { signal: withdraw.signal },
        )
        return { answers: completeAnswers(request, result) }
      } catch (error) {
        if (error instanceof RpcFault && request.signal?.aborted !== true) {
          if (error.code === ErrorCode.declined) throw await questionError(host, 'ASK_CANCELLED', 'The person did not answer.')
          if (error.code === ErrorCode.methodNotFound) return next()
        }
        throw error
      } finally {
        request.signal?.removeEventListener('abort', onAbort)
      }
    }
    host.ctx.on(
      'user-questions/request',
      (async (request: AskRequest, next: () => Promise<Answer>) => {
        try {
          return await answer(request, next)
        } catch (error) {
          if (error instanceof RpcFault) throw await questionError(host, 'ASK_FAILED', error.message)
          throw error
        }
      }) as never,
      { global: true, prepend: true },
    )
  },
}

/** One answer for every question, in question order: an unanswered one is skipped, never invented. */
function completeAnswers(request: AskRequest, result: QuestionAskResult): QuestionAnswer[] {
  const given = new Map((result?.answers ?? []).map((a) => [a.id, a]))
  return request.questions.map((q) => {
    const a = given.get(q.id)
    if (a === undefined) return { id: q.id, selected: [] }
    const out: QuestionAnswer = { id: q.id, selected: Array.isArray(a.selected) ? a.selected.map(String) : [] }
    if (typeof a.custom === 'string' && a.custom.length > 0) out.custom = a.custom
    return out
  })
}

/** Sends the plan for review and turns the decision into the answer dsh's exit tool reads. */
async function planReview(
  host: ExtraHost,
  request: AskRequest,
  review: AskRequest['questions'][number],
  sessionId: string,
  callId: string,
  signal: AbortSignal,
): Promise<Answer> {
  const approve = review.intent?.approve ?? review.options?.[0]?.label ?? 'Approve'
  const keep = review.options?.find((o) => o.label !== approve)?.label ?? 'Keep planning'
  const result = await host.channel.request<PlanReviewResult>(
    Method.planReview,
    { sessionId, callId, plan: review.detail ?? review.question },
    { signal },
  )
  const decision: PlanDecision = result?.decision ?? 'dismiss'
  if (decision === 'proceed') return { answers: [{ id: review.id, selected: [approve] }] }
  if (decision === 'keep_planning') {
    const feedback = typeof result.feedback === 'string' ? result.feedback.trim() : ''
    return { answers: [{ id: review.id, selected: feedback === '' ? [keep] : [], ...(feedback === '' ? {} : { custom: feedback }) }] }
  }
  throw await questionError(host, 'ASK_CANCELLED', 'The plan review was dismissed.')
}
