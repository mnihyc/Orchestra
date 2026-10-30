import { StringDecoder } from "node:string_decoder";
import { PassThrough, Writable } from "node:stream";
import { randomUUID } from "node:crypto";
import { copyFile, readFile, readdir, realpath, stat } from "node:fs/promises";
import path from "node:path";

//#region src/protocol.ts
/** The name the plugin announces itself with. */
const PLUGIN_NAME = "orch-dsh-bridge";
/** The prefix of every method of this protocol. */
const METHOD_PREFIX = "_orch/";
/** The methods and notifications of the protocol, by name. */
const Method = {
	/** Plugin to Orch notification after `initialize`; also an Orch to plugin request that answers the same body. */
	hello: "_orch/hello",
	/** Plugin to Orch notification: one extra changed state at run time. */
	extraState: "_orch/extra_state",
	/** Plugin to Orch notification: live deltas of one model call. */
	stream: "_orch/stream",
	/** Orch to plugin request: steer the running turn. */
	inputSteer: "_orch/input/steer",
	/** Orch to plugin request: queue a follow-up for after the running turn. */
	inputFollowup: "_orch/input/followup",
	/** Plugin to Orch notification: what became of an input message. */
	inputState: "_orch/input/state",
	/** Plugin to Orch request: ask a person questions. */
	questionAsk: "_orch/question/ask",
	/** Plugin to Orch notification: a question asked earlier is no longer waited for. */
	questionClosed: "_orch/question/closed",
	/** Orch to plugin request: switch plan mode on or off. */
	planSet: "_orch/plan/set",
	/** Orch to plugin request: read plan mode. */
	planGet: "_orch/plan/get",
	/** Plugin to Orch notification: plan mode changed. */
	planState: "_orch/plan/state",
	/** Plugin to Orch request: the model submitted a plan for review. */
	planReview: "_orch/plan/review",
	/** Plugin to Orch notification: a tool call was refused because plan mode is on. */
	planRefused: "_orch/plan/refused",
	/** Orch to plugin request: choose whether changes ask for approval (`ask`) or run (`bypass`). */
	approvalSet: "_orch/approval/set",
	/** Plugin to Orch notification: token accounting of one model call. */
	usage: "_orch/usage",
	/** Plugin to Orch notification: a phase of a compaction. */
	compaction: "_orch/compaction",
	/** Orch to plugin request: compact the session now. */
	compactionRun: "_orch/compaction/run",
	/** Orch to plugin request: rescan the skills root. */
	skillsRefresh: "_orch/skills/refresh",
	/** Orch to plugin request: copy a session at a turn boundary. */
	sessionFork: "_orch/session/fork",
	/** Orch to plugin request: write a neutral history into a new session. */
	historyImport: "_orch/history/import",
	/** Orch to plugin request: write a session's own record to a file. */
	historyExport: "_orch/history/export"
};
/** JSON-RPC error codes the protocol uses beyond the standard ones. */
const ErrorCode = {
	parseError: -32700,
	invalidRequest: -32600,
	methodNotFound: -32601,
	invalidParams: -32602,
	internalError: -32603,
	/** The person, or Orch for them, declined to answer. */
	declined: -32001,
	/** The extra behind the method is off in this process. */
	extraOff: -32002,
	/** The session is unknown or not live in this process. */
	unknownSession: -32003,
	/** The request cannot be done now (for example, the turn already ended). */
	notNow: -32004
};
/** Every extra, in the order the plugin reports them. */
const EXTRA_IDS = [
	"streaming",
	"steering",
	"questions",
	"plan_mode",
	"approvals",
	"usage",
	"compaction",
	"skills",
	"fork",
	"native_carry",
	"executor_backends"
];
/** The key of `_meta` in a committed `session/update` that ties it to the call that streamed it. */
const COMMIT_META_KEY = "orch";
/** The format of Orch's neutral history: JSON lines of items, the main stream, oldest first. */
const NEUTRAL_FORMAT = "orch-neutral-jsonl-v1";
/** The format of dsh's own record of a session. */
const NATIVE_FORMAT = "dsh-session-v4";
/** The key of `_meta` that holds Orch's members. */
const META_KEY = "orch";
/** Whether a message is a request or notification (it names a method). */
function isRequest(message) {
	return typeof message.method === "string";
}
/** Whether a method belongs to this protocol. */
function isOrchMethod(method) {
	return method.startsWith(METHOD_PREFIX);
}

//#endregion
//#region src/channel.ts
/**
* The line channel on dsh's stdio.
*
* dsh's ACP server reads protocol lines from its stdin and writes them to its stdout. This module stands between those
* two streams and the real ones. Lines for the plugin's own methods (`_orch/*`) and the answers to the plugin's own
* requests are taken out of what the server reads; a few standard methods the plugin answers itself (plan mode's
* `session/set_mode`) likewise; everything else reaches the server unchanged. Everything the server writes passes the
* plugin's taps (which may add a member to a line or drop one) and leaves through one ordered writer together with the
* plugin's own messages, so Orch reads events in the order they happened and no two writes ever meet inside a line.
*/
/** The id prefix of this plugin's own requests, which cannot collide with the ids dsh's server issues. */
const OWN_ID_PREFIX = "orch-";
/** A failure with a JSON-RPC code, for handlers to throw. */
var RpcFault = class extends Error {
	code;
	data;
	constructor(code, message, data) {
		super(message);
		this.name = "RpcFault";
		this.code = code;
		this.data = data;
	}
};
/** The channel; make one per process. */
var Channel = class {
	handlers = /* @__PURE__ */ new Map();
	interceptors = /* @__PURE__ */ new Map();
	pending = /* @__PURE__ */ new Map();
	outgoingTaps = [];
	afterWriteTaps = [];
	incomingTaps = [];
	closeListeners = [];
	nextId = 1;
	closed = false;
	io;
	/** What dsh's server reads as its stdin. */
	serverInput = new PassThrough();
	/** What dsh's server writes to as its stdout. */
	serverOutput;
	constructor(io) {
		this.io = io;
		let held = "";
		const decoder = new StringDecoder("utf8");
		this.serverOutput = new Writable({ write: (chunk, _encoding, done) => {
			held += typeof chunk === "string" ? chunk : decoder.write(chunk);
			let newline = held.indexOf("\n");
			let flushed = void 0;
			while (newline >= 0) {
				const line = held.slice(0, newline).trim();
				held = held.slice(newline + 1);
				if (line.length > 0) flushed = this.fromServer(line) ?? flushed;
				newline = held.indexOf("\n");
			}
			if (flushed === void 0) done();
			else flushed.then(() => done(), (error) => done(error));
		} });
		this.listen();
	}
	/**
	* The same two server-side streams as the message stream dsh's ACP server accepts in its configuration
	* (`stream`), for a profile that hands it over explicitly. Nothing reads `serverInput` until the server first reads
	* this stream, so a server that reads `process.stdin` instead is not robbed of its lines.
	*/
	asMessageStream() {
		const input = this.serverInput;
		const decoder = new StringDecoder("utf8");
		let attached = false;
		return {
			readable: new ReadableStream({ pull: (controller) => new Promise((resolve) => {
				if (!attached) {
					attached = true;
					let held = "";
					input.on("data", (chunk) => {
						held += typeof chunk === "string" ? chunk : decoder.write(chunk);
						let newline = held.indexOf("\n");
						while (newline >= 0) {
							const line = held.slice(0, newline).trim();
							held = held.slice(newline + 1);
							if (line.length > 0) controller.enqueue(JSON.parse(line));
							newline = held.indexOf("\n");
						}
					});
					input.once("end", () => controller.close());
				}
				resolve();
			}) }, { highWaterMark: 0 }),
			writable: new WritableStream({ write: (message) => new Promise((resolve, reject) => {
				this.serverOutput.write(`${JSON.stringify(message)}\n`, (error) => error ? reject(error) : resolve());
			}) })
		};
	}
	/** Answers the requests and notifications of one `_orch/*` method. */
	handle(method, handler) {
		this.handlers.set(method, handler);
	}
	/** Answers a standard method itself, so that it never reaches dsh's server. */
	intercept(method, handler) {
		this.interceptors.set(method, handler);
	}
	/** The `_orch/*` methods with a handler, sorted. */
	handledMethods() {
		return [...this.handlers.keys()].sort();
	}
	/** Looks at every message that is read for the server, before it is passed on. Must be quick and must not throw. */
	tapIncoming(tap) {
		this.incomingTaps.push(tap);
	}
	/**
	* Looks at every message the server writes before it leaves; returns the message, a changed one, or null to drop
	* it. Must be quick and must not throw.
	*/
	tapOutgoing(tap) {
		this.outgoingTaps.push(tap);
	}
	/** Called after a message of the server has been written to the wire. */
	afterWrite(tap) {
		this.afterWriteTaps.push(tap);
	}
	/** Called once when the input ends. */
	onClose(listener) {
		this.closeListeners.push(listener);
	}
	/** Writes a notification. */
	notify(method, params) {
		this.writeLine({
			jsonrpc: "2.0",
			method,
			params
		});
	}
	/** Sends a request and waits for the answer; rejects with `RpcFault` for an error response. */
	request(method, params, options = {}) {
		if (this.closed) return Promise.reject(new RpcFault(ErrorCode.notNow, "the channel is closed"));
		const id = `${OWN_ID_PREFIX}${this.nextId++}`;
		return new Promise((resolve, reject) => {
			const signal = options.signal;
			const onAbort = () => {
				this.pending.delete(id);
				this.notify("$/cancel_request", { requestId: id });
				reject(new RpcFault(ErrorCode.notNow, `${method} was withdrawn`));
			};
			const detach = () => signal?.removeEventListener("abort", onAbort);
			if (signal?.aborted) {
				reject(new RpcFault(ErrorCode.notNow, `${method} was withdrawn`));
				return;
			}
			signal?.addEventListener("abort", onAbort, { once: true });
			this.pending.set(id, {
				resolve,
				reject,
				method,
				detach
			});
			this.writeLine({
				jsonrpc: "2.0",
				id,
				method,
				params
			});
		});
	}
	/** Whether the input has ended. */
	get isClosed() {
		return this.closed;
	}
	listen() {
		const decoder = new StringDecoder("utf8");
		let buffer = "";
		const onData = (chunk) => {
			buffer += typeof chunk === "string" ? chunk : decoder.write(chunk);
			let newline = buffer.indexOf("\n");
			while (newline >= 0) {
				const line = buffer.slice(0, newline);
				buffer = buffer.slice(newline + 1);
				if (line.trim().length > 0) this.receive(line);
				newline = buffer.indexOf("\n");
			}
		};
		const onEnd = () => {
			const rest = buffer + decoder.end();
			buffer = "";
			if (rest.trim().length > 0) this.receive(rest);
			this.close();
		};
		this.io.input.on("data", onData);
		this.io.input.once("end", onEnd);
		this.io.input.once("close", onEnd);
	}
	receive(line) {
		let message;
		try {
			message = JSON.parse(line);
		} catch {
			this.io.log(`orch-dsh-bridge: a line of the input is not JSON (${line.length} bytes), skipped`);
			return;
		}
		if (typeof message !== "object" || message === null) {
			this.io.log("orch-dsh-bridge: a line of the input is not an object, skipped");
			return;
		}
		if (isRequest(message)) {
			if (isOrchMethod(message.method)) {
				this.dispatch(this.handlers.get(message.method), message);
				return;
			}
			for (const tap of this.incomingTaps) tap(message);
			const interceptor = this.interceptors.get(message.method);
			if (interceptor !== void 0) {
				this.dispatch(interceptor, message);
				return;
			}
		} else if (typeof message.id === "string" && message.id.startsWith("orch-")) {
			this.settle(message);
			return;
		} else for (const tap of this.incomingTaps) tap(message);
		this.serverInput.write(`${line}\n`);
	}
	settle(response) {
		const id = String(response.id);
		const pending = this.pending.get(id);
		if (pending === void 0) return;
		this.pending.delete(id);
		pending.detach();
		if (response.error !== void 0) pending.reject(new RpcFault(response.error.code, response.error.message, response.error.data));
		else pending.resolve(response.result);
	}
	async dispatch(handler, request) {
		const wantsAnswer = request.id !== void 0 && request.id !== null;
		if (handler === void 0) {
			if (wantsAnswer) this.respondError(request.id, ErrorCode.methodNotFound, `${request.method} is not known`);
			return;
		}
		try {
			const result = await handler(request.params, request);
			if (wantsAnswer) this.writeLine({
				jsonrpc: "2.0",
				id: request.id,
				result: result ?? {}
			});
		} catch (error) {
			if (!wantsAnswer) {
				this.io.log(`orch-dsh-bridge: ${request.method} failed: ${errorText(error)}`);
				return;
			}
			if (error instanceof RpcFault) this.respondError(request.id, error.code, error.message, error.data);
			else this.respondError(request.id, ErrorCode.internalError, errorText(error));
		}
	}
	respondError(id, code, message, data) {
		const error = data === void 0 ? {
			code,
			message
		} : {
			code,
			message,
			data
		};
		this.writeLine({
			jsonrpc: "2.0",
			id,
			error
		});
	}
	/** A line the server wrote. Returns a promise when the output asks the server to wait. */
	fromServer(line) {
		let message;
		try {
			message = JSON.parse(line);
		} catch {
			this.io.log(line);
			return;
		}
		let out = message;
		for (const tap of this.outgoingTaps) {
			if (out === null) return void 0;
			out = tap(out);
		}
		if (out === null) return void 0;
		const flushed = this.writeLine(out);
		for (const tap of this.afterWriteTaps) tap(out);
		return flushed;
	}
	/** Writes one message as one line. Returns a promise only when the output asks the writer to wait. */
	writeLine(message) {
		const line = `${JSON.stringify(message)}\n`;
		if (this.io.output.write(line)) return void 0;
		return new Promise((resolve) => {
			this.io.output.once("drain", resolve);
		});
	}
	close() {
		if (this.closed) return;
		this.closed = true;
		for (const [id, pending] of this.pending) {
			pending.detach();
			pending.reject(new RpcFault(ErrorCode.notNow, `${pending.method} was not answered before the input ended`));
			this.pending.delete(id);
		}
		this.serverInput.end();
		for (const listener of this.closeListeners) try {
			listener();
		} catch (error) {
			this.io.log(`orch-dsh-bridge: a close listener failed: ${errorText(error)}`);
		}
	}
};
/** The text of anything that was thrown. */
function errorText(error) {
	if (error instanceof Error) return error.message;
	return typeof error === "string" ? error : JSON.stringify(error);
}

//#endregion
//#region src/host.ts
/** The dsh builds the extras were written against and are tested on. */
const TESTED_HOST_VERSIONS = ["0.2.1-rc.2", "0.2.1-rc.4"];
/** Why an extra cannot start; thrown from `start`, caught by the registry, which turns the extra off. */
var ExtraOff = class extends Error {
	reason;
	detail;
	constructor(reason, detail) {
		super(detail ?? reason);
		this.name = "ExtraOff";
		this.reason = reason;
		this.detail = detail;
	}
};
/** Polls `read` until it returns something, or the wait is over. */
async function waitFor(read, ms, stepMs = 20) {
	const deadline = Date.now() + ms;
	for (;;) {
		const value = read();
		if (value !== void 0) return value;
		if (Date.now() >= deadline) return void 0;
		await new Promise((resolve) => setTimeout(resolve, stepMs));
	}
}
/** The version of the dsh build, or null when it cannot be read from inside the process. */
async function readHostVersion() {
	try {
		const version = (await import("@deepseek-ai/dsh-app-boot")).getDshRuntimeVersion?.();
		return typeof version === "string" ? version : null;
	} catch {
		return null;
	}
}

//#endregion
//#region src/session-book.ts
const OPENING = /* @__PURE__ */ new Set([
	"session/new",
	"session/resume",
	"session/load"
]);
/** The `_meta.orch` object of a message's params, or undefined. */
function orchMeta(params) {
	const meta = params?._meta?.[META_KEY];
	return typeof meta === "object" && meta !== null ? meta : void 0;
}
/** The book. */
var SessionBook = class {
	metas = /* @__PURE__ */ new Map();
	waiting = /* @__PURE__ */ new Map();
	opened = [];
	prompted = [];
	/** Watches a channel's traffic. */
	attach(channel) {
		channel.tapIncoming((message) => this.incoming(message));
		channel.tapOutgoing((message) => this.outgoing(message));
	}
	/** What Orch last told of a session; members are replaced as newer messages bring them. */
	metaOf(sessionId) {
		return this.metas.get(sessionId);
	}
	/** Called with the answer to every request that opens a session, before it leaves. */
	onOpened(hook) {
		this.opened.push(hook);
	}
	/** Called with every prompt, before dsh's server reads it. */
	onPrompt(hook) {
		this.prompted.push(hook);
	}
	remember(sessionId, meta) {
		if (meta !== void 0) this.metas.set(sessionId, {
			...this.metas.get(sessionId),
			...meta
		});
	}
	incoming(message) {
		if (!isRequest(message)) return;
		const params = message.params;
		const sessionId = typeof params?.sessionId === "string" ? params.sessionId : void 0;
		if (OPENING.has(message.method) && message.id !== void 0) {
			const entry = { method: message.method };
			if (sessionId !== void 0) entry.sessionId = sessionId;
			const meta = orchMeta(message.params);
			if (meta !== void 0) entry.meta = meta;
			this.waiting.set(message.id, entry);
		} else if (message.method === "session/prompt" && sessionId !== void 0) {
			const meta = orchMeta(message.params);
			this.remember(sessionId, meta);
			for (const hook of this.prompted) hook(sessionId, meta);
		}
	}
	outgoing(message) {
		if (isRequest(message) || message.id === null) return message;
		const entry = this.waiting.get(message.id);
		if (entry === void 0) return message;
		this.waiting.delete(message.id);
		const result = message.result;
		if (message.error !== void 0 || result === void 0) return message;
		const sessionId = entry.sessionId ?? (typeof result["sessionId"] === "string" ? result["sessionId"] : void 0);
		if (sessionId === void 0) return message;
		this.remember(sessionId, entry.meta);
		for (const hook of this.opened) hook({
			method: entry.method,
			sessionId,
			result
		});
		return message;
	}
};

//#endregion
//#region src/bridge.ts
/**
* The bridge: runs the extras, keeps what Orch said of each session, and announces what works.
*
* It has no side effects of its own when it is made; `takeover.ts` and `index.ts` wire it to the process.
*/
/** The plugin's version, announced in the hello. */
const PLUGIN_VERSION = "0.1.0";
/** How long start waits for the extras beyond what each extra itself waits, in milliseconds. */
const START_SLACK_MS = 2e3;
/** Notifications and requests the plugin may send (announced in the hello). */
const EMITS = [
	Method.hello,
	Method.extraState,
	Method.stream,
	Method.inputState,
	Method.questionAsk,
	Method.questionClosed,
	Method.planState,
	Method.planReview,
	Method.planRefused,
	Method.usage,
	Method.compaction,
	"elicitation/create"
];
/** Runs the extras and answers the hello; one per process. */
var Bridge = class {
	channel;
	book = new SessionBook();
	ctx;
	config;
	extras;
	log;
	states = /* @__PURE__ */ new Map();
	shareds = /* @__PURE__ */ new Map();
	hostVersion = null;
	ready = Promise.resolve();
	helloSent = false;
	initializeId = void 0;
	initializeAnswered = false;
	constructor(ctx, config, extras, channel, log) {
		this.ctx = ctx;
		this.config = config;
		this.extras = extras;
		this.log = log;
		this.channel = channel;
		this.book.attach(channel);
		channel.handle(Method.hello, () => this.hello());
		channel.tapIncoming((message) => {
			if (isRequest(message) && message.method === "initialize") this.initializeId = message.id;
		});
		channel.afterWrite((message) => this.afterServerWrite(message));
	}
	/** Starts every extra; resolves when each is on or off. */
	start() {
		this.ready = this.startAll();
		return this.ready;
	}
	/** The state of every extra now. */
	hello() {
		const extras = {};
		for (const id of EXTRA_IDS) extras[id] = this.states.get(id) ?? {
			status: "off",
			reason: "self_check_failed",
			detail: "it did not start"
		};
		return {
			protocol: 1,
			plugin: {
				name: PLUGIN_NAME,
				version: PLUGIN_VERSION
			},
			host: {
				name: "dsh",
				version: this.hostVersion,
				tested: this.hostVersion !== null && TESTED_HOST_VERSIONS.includes(this.hostVersion)
			},
			extras,
			methods: this.channel.handledMethods(),
			emits: [...EMITS]
		};
	}
	async startAll() {
		this.hostVersion = this.config.hostVersion ?? await readHostVersion();
		const host = this.makeHost();
		for (const extra of this.extras) {
			if (!this.config.extras[extra.id]) {
				this.states.set(extra.id, {
					status: "off",
					reason: "disabled_by_settings"
				});
				continue;
			}
			await this.startOne(extra, host);
		}
		this.sendHelloWhenPossible();
	}
	async startOne(extra, host) {
		let timer;
		try {
			await Promise.race([Promise.resolve(extra.start(host)), new Promise((_, reject) => {
				timer = setTimeout(() => reject(new ExtraOff("self_check_failed", "it did not finish starting")), this.config.serviceWaitMs + START_SLACK_MS);
			})]);
			this.states.set(extra.id, { status: "on" });
		} catch (error) {
			const state = error instanceof ExtraOff ? error.detail === void 0 ? {
				status: "off",
				reason: error.reason
			} : {
				status: "off",
				reason: error.reason,
				detail: error.detail
			} : {
				status: "off",
				reason: "self_check_failed",
				detail: errorText(error)
			};
			this.states.set(extra.id, state);
			this.log(`orch-dsh-bridge: ${extra.id} is off: ${state.status === "off" ? state.detail ?? state.reason : ""}`);
		} finally {
			if (timer !== void 0) clearTimeout(timer);
		}
	}
	makeHost() {
		const host = {
			ctx: this.ctx,
			channel: this.channel,
			config: this.config,
			book: this.book,
			service: (name) => this.ctx.get(name) ?? void 0,
			waitService: (name) => waitFor(() => this.ctx.get(name) ?? void 0, this.config.serviceWaitMs),
			isOn: (id) => this.states.get(id)?.status === "on" || !this.states.has(id),
			failRuntime: (id, error) => {
				if (this.states.get(id)?.status === "off") return;
				const state = {
					status: "off",
					reason: "runtime_failure",
					detail: errorText(error)
				};
				this.states.set(id, state);
				this.log(`orch-dsh-bridge: ${id} is off: ${errorText(error)}`);
				this.channel.notify(Method.extraState, {
					extra: id,
					state
				});
			},
			guarded: (id, fn) => (...args) => {
				if (!host.isOn(id)) return;
				try {
					fn(...args);
				} catch (error) {
					host.failRuntime(id, error);
				}
			},
			shared: (key, make) => {
				if (!this.shareds.has(key)) this.shareds.set(key, make());
				return this.shareds.get(key);
			},
			log: this.log
		};
		return host;
	}
	afterServerWrite(message) {
		if (this.initializeAnswered || this.initializeId === void 0) return;
		if (!isRequest(message) && message.id === this.initializeId) {
			this.initializeAnswered = true;
			this.sendHelloWhenPossible();
		}
	}
	sendHelloWhenPossible() {
		if (this.helloSent || !this.initializeAnswered) return;
		this.ready.then(() => {
			if (this.helloSent) return;
			this.helloSent = true;
			this.channel.notify(Method.hello, this.hello());
		});
	}
};

//#endregion
//#region src/config.ts
/**
* The plugin's settings, as Orch writes them into the row's `config` of the patch it starts dsh with.
* Every number is a default that Orch can change; none is a limit of the protocol.
*/
/** Plugin defaults. */
const DEFAULTS = {
	extras: Object.fromEntries(EXTRA_IDS.map((id) => [id, true])),
	streaming: {
		flushMs: 25,
		flushChars: 4096
	},
	planMode: {
		readTools: [
			"read",
			"glob",
			"grep",
			"ls",
			"web_search",
			"web_fetch",
			"skill",
			"todo_write",
			"ask_user_question",
			"exit_plan_mode",
			"read_image"
		],
		commandTool: "bash",
		readCommands: [
			"ls",
			"cat",
			"head",
			"tail",
			"wc",
			"file",
			"stat",
			"pwd",
			"echo",
			"grep",
			"rg",
			"find",
			"tree",
			"du",
			"df",
			"which",
			"env",
			"git status",
			"git log",
			"git diff",
			"git show",
			"git branch",
			"git ls-files",
			"git rev-parse"
		]
	},
	approvals: {
		mode: "ask",
		askTools: [
			"bash",
			"pwsh",
			"write",
			"edit",
			"str_replace_editor"
		]
	},
	backends: {
		enabled: false,
		url: null,
		tokenEnv: "ORCH_EXECUTOR_TOKEN",
		jobWaitMs: 25e3,
		globMax: 100,
		grepMax: 250
	},
	guardStdout: true,
	hostVersion: null,
	serviceWaitMs: 5e3
};
function record(value, where) {
	if (value === void 0 || value === null) return {};
	if (typeof value !== "object" || Array.isArray(value)) throw new Error(`orch-dsh-bridge: ${where} must be an object`);
	return value;
}
function positive(value, fallback, where) {
	if (value === void 0) return fallback;
	if (typeof value !== "number" || !Number.isFinite(value) || value < 0) throw new Error(`orch-dsh-bridge: ${where} must be a number of zero or more`);
	return value;
}
function strings(value, fallback, where) {
	if (value === void 0) return fallback;
	if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) throw new Error(`orch-dsh-bridge: ${where} must be a list of strings`);
	return value;
}
/** Reads the row's config; a wrong type throws with the name of the field. */
function resolveConfig(raw) {
	const root = record(raw, "config");
	const extras = { ...DEFAULTS.extras };
	const given = record(root["extras"], "extras");
	for (const [key, value] of Object.entries(given)) {
		if (!EXTRA_IDS.includes(key)) throw new Error(`orch-dsh-bridge: extras.${key} is not an extra`);
		if (typeof value !== "boolean") throw new Error(`orch-dsh-bridge: extras.${key} must be true or false`);
		extras[key] = value;
	}
	const streaming = record(root["streaming"], "streaming");
	const plan = record(root["planMode"], "planMode");
	const approvals = record(root["approvals"], "approvals");
	const mode = approvals["mode"];
	if (mode !== void 0 && mode !== "ask" && mode !== "bypass") throw new Error("orch-dsh-bridge: approvals.mode must be 'ask' or 'bypass'");
	const backends = record(root["backends"], "backends");
	const flag = backends["enabled"];
	if (flag !== void 0 && typeof flag !== "boolean") throw new Error("orch-dsh-bridge: backends.enabled must be true or false");
	const backendUrl = backends["url"];
	if (backendUrl !== void 0 && backendUrl !== null && typeof backendUrl !== "string") throw new Error("orch-dsh-bridge: backends.url must be a string");
	const tokenEnv = backends["tokenEnv"];
	if (tokenEnv !== void 0 && typeof tokenEnv !== "string") throw new Error("orch-dsh-bridge: backends.tokenEnv must be a string");
	const guard = root["guardStdout"];
	if (guard !== void 0 && typeof guard !== "boolean") throw new Error("orch-dsh-bridge: guardStdout must be true or false");
	const commandTool = plan["commandTool"];
	if (commandTool !== void 0 && typeof commandTool !== "string") throw new Error("orch-dsh-bridge: planMode.commandTool must be a string");
	return {
		extras,
		streaming: {
			flushMs: positive(streaming["flushMs"], DEFAULTS.streaming.flushMs, "streaming.flushMs"),
			flushChars: positive(streaming["flushChars"], DEFAULTS.streaming.flushChars, "streaming.flushChars")
		},
		planMode: {
			readTools: strings(plan["readTools"], DEFAULTS.planMode.readTools, "planMode.readTools"),
			commandTool: commandTool ?? DEFAULTS.planMode.commandTool,
			readCommands: strings(plan["readCommands"], DEFAULTS.planMode.readCommands, "planMode.readCommands")
		},
		approvals: {
			mode: mode ?? DEFAULTS.approvals.mode,
			askTools: strings(approvals["askTools"], DEFAULTS.approvals.askTools, "approvals.askTools")
		},
		backends: {
			enabled: flag ?? DEFAULTS.backends.enabled,
			url: backendUrl ?? DEFAULTS.backends.url,
			tokenEnv: tokenEnv ?? DEFAULTS.backends.tokenEnv,
			jobWaitMs: positive(backends["jobWaitMs"], DEFAULTS.backends.jobWaitMs, "backends.jobWaitMs"),
			globMax: positive(backends["globMax"], DEFAULTS.backends.globMax, "backends.globMax"),
			grepMax: positive(backends["grepMax"], DEFAULTS.backends.grepMax, "backends.grepMax")
		},
		guardStdout: guard ?? DEFAULTS.guardStdout,
		hostVersion: typeof root["hostVersion"] === "string" ? root["hostVersion"] : null,
		serviceWaitMs: positive(root["serviceWaitMs"], DEFAULTS.serviceWaitMs, "serviceWaitMs")
	};
}

//#endregion
//#region src/extras/commands.ts
/**
* Whether a shell command only reads. Plan mode runs only such commands, and approvals do not ask for them.
*
* The check is deliberately narrow: a command passes only when every part of it is a command from the list, and
* nothing in it can write, run another program, or substitute output. When in doubt it says no.
*/
/** Characters and words that make a segment more than a plain read. */
const WRITES_OR_SUBSTITUTES = /`|\$\(|<\(|>\(|\$\{|\bsudo\b|\beval\b|\bexec\b|\bxargs\b|\btee\b/;
/** A redirection that writes somewhere other than the bit bucket or another stream. */
function writesWithRedirect(segment) {
	const withoutSafe = segment.replace(/\d?>&\d/g, " ").replace(/\d?>>?\s*\/dev\/null/g, " ");
	return />/.test(withoutSafe);
}
/** Flags that turn a read command into a writer. */
function hasWritingFlag(words) {
	const joined = ` ${words.join(" ")} `;
	if (words[0] === "find") return /\s-(exec|execdir|ok|okdir|delete|fprint|fprintf|fls)\b/.test(joined);
	if (words[0] === "rg") return /\s--(pre|pre-glob)\b/.test(joined);
	if (words[0] === "sed") return /\s-[a-zA-Z]*i/.test(joined);
	if (words[0] === "git") return /\s--(output|ext-diff|exec-path)\b|\s-o\b/.test(joined);
	return false;
}
/** Splits a command line into its simple commands at `&&`, `||`, `;`, `|` and newlines. */
function segmentsOf(command) {
	return command.split(/&&|\|\||;|\||\n/).map((part) => part.trim()).filter((part) => part.length > 0);
}
/** Drops leading `NAME=value` words and `env`, which only set variables for the command after them. */
function stripEnvironment(words) {
	let at = 0;
	while (at < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[at] ?? "")) at += 1;
	if (words[at] === "env") {
		at += 1;
		while (at < words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[at] ?? "")) at += 1;
	}
	return words.slice(at);
}
/**
* True when `command` only reads.
* @param command - the shell command line
* @param readCommands - first words (or first two words, for `git status`) that only read
*/
function isReadOnlyCommand(command, readCommands) {
	if (command.trim().length === 0) return false;
	const allowed = new Set(readCommands);
	for (const segment of segmentsOf(command)) {
		if (WRITES_OR_SUBSTITUTES.test(segment) || writesWithRedirect(segment)) return false;
		const words = stripEnvironment(segment.split(/\s+/));
		if (words.length === 0) continue;
		const first = words[0] ?? "";
		const pair = `${first} ${words[1] ?? ""}`.trim();
		if (!allowed.has(first) && !allowed.has(pair)) return false;
		if (hasWritingFlag(words)) return false;
	}
	return true;
}

//#endregion
//#region src/extras/shared.ts
/** Small helpers the extras share. */
/** The session a client opened that a session belongs to: a sub-agent's work is reported under the session that started it. */
function rootSessionId(host, session) {
	const sessions = host.service("sessions");
	let at = session;
	for (let hops = 0; hops < 16; hops += 1) {
		const parent = at.header.parentSession;
		const next = parent === void 0 ? void 0 : sessions?.get(parent);
		if (next === void 0) break;
		at = next;
	}
	return at.header.id;
}
/** The session id of the object a listener was given: a session, or an agent. */
function sessionIdOf(value) {
	return value?.header?.id ?? value?.id;
}
/** The live agent of a session, or a fault the caller sends back. */
function liveAgent(host, sessionId) {
	if (typeof sessionId !== "string") throw new RpcFault(ErrorCode.invalidParams, "sessionId must be a string");
	const agent = host.service("agents")?.get(sessionId);
	if (agent === void 0) throw new RpcFault(ErrorCode.unknownSession, `session ${sessionId} is not live in this process`);
	return agent;
}
/** Refuses a request of an extra that is off. */
function requireOn(host, extra) {
	if (!host.isOn(extra)) throw new RpcFault(ErrorCode.extraOff, `${extra} is off in this process`);
}
/** Reads params as an object. */
function paramsOf(params) {
	if (typeof params !== "object" || params === null || Array.isArray(params)) throw new RpcFault(ErrorCode.invalidParams, "params must be an object");
	return params;
}
/** A required string param. */
function stringParam(params, key) {
	const value = params[key];
	if (typeof value !== "string" || value.length === 0) throw new RpcFault(ErrorCode.invalidParams, `${key} must be a non-empty string`);
	return value;
}

//#endregion
//#region src/extras/plan.ts
/**
* Plan mode, enforced.
*
* dsh's plan mode is guidance to the model: a prompt section and a tool that presents the plan. Nothing stops a
* model that ignores it. This extra turns it into a rule. It lets Orch switch plan mode on and off, refuses at the
* tool pipeline every call that is not on a read-only list (and runs only read-only commands), and reports the state
* and each refusal. The plan itself reaches Orch through the question extra as `_orch/plan/review`.
*/
/** The same policy is checked before asking and again at the host's monotonic execution guard. */
function planToolGuard(host) {
	return host.shared("plan-tool-guard", () => ({ refuse: (_call) => void 0 }));
}
/** The sentence the model is given when plan mode refuses a call. */
function refusalFor(tool, command) {
	return `${command === void 0 ? `The ${tool} tool` : `This command (${command.trim().split(/\s+/)[0] ?? tool})`} would change something, and plan mode is on: only reading is allowed. Explore with read-only tools, then present the plan with exit_plan_mode.`;
}
const plan = {
	id: "plan_mode",
	async start(host) {
		const service = await host.waitService("planMode");
		if (service === void 0) throw new ExtraOff("host_api_missing", "the plan-mode service is not loaded");
		if (typeof service.get !== "function" || typeof service.set !== "function") throw new ExtraOff("host_api_changed", "the plan-mode service has no get and set");
		const tools = host.service("tools");
		if (typeof tools?.guard !== "function") throw new ExtraOff("host_api_missing", "the tool pipeline has no guards");
		const { readTools, commandTool, readCommands } = host.config.planMode;
		const allowed = new Set(readTools);
		const policy = planToolGuard(host);
		policy.refuse = (call) => {
			if (!host.isOn("plan_mode")) return void 0;
			const agent = call.agent;
			const sessionId = sessionIdOf(agent);
			if (agent === void 0 || sessionId === void 0) return void 0;
			let active = false;
			try {
				active = service.get(agent).active;
			} catch (error) {
				host.failRuntime("plan_mode", error);
				return;
			}
			if (!active) return void 0;
			if (allowed.has(call.name)) return void 0;
			let command;
			if (call.name === commandTool) {
				const given = call.arguments?.command;
				command = typeof given === "string" ? given : "";
				if (isReadOnlyCommand(command, readCommands)) return void 0;
			}
			const reason = refusalFor(call.name, command);
			host.channel.notify(Method.planRefused, {
				sessionId,
				callId: call.callId,
				tool: call.name,
				reason
			});
			return reason;
		};
		tools.guard((call) => policy.refuse(call));
		const stateOf = (sessionId, agent) => {
			const now = service.get(agent);
			const state = {
				sessionId,
				active: now.active
			};
			if (now.pending === true) state.pending = true;
			return state;
		};
		host.book.onPrompt(host.guarded("plan_mode", (sessionId, meta) => {
			if (typeof meta?.plan !== "boolean") return;
			const agent = host.service("agents")?.get(sessionId);
			if (agent !== void 0 && service.get(agent).active !== meta.plan) {
				service.set(agent, meta.plan);
				host.channel.notify(Method.planState, stateOf(sessionId, agent));
			}
		}));
		host.channel.handle(Method.planSet, (params) => {
			requireOn(host, "plan_mode");
			const input = paramsOf(params);
			const agent = liveAgent(host, input.sessionId);
			const outcome = service.set(agent, input.active === true);
			host.channel.notify(Method.planState, stateOf(agent.id, agent));
			return { outcome };
		});
		host.channel.handle(Method.planGet, (params) => {
			requireOn(host, "plan_mode");
			const input = paramsOf(params);
			const agent = liveAgent(host, input.sessionId);
			return stateOf(agent.id, agent);
		});
		host.ctx.on("session/event", host.guarded("plan_mode", (session, event) => {
			if (event.type !== "plan/mode") return;
			const sessionId = sessionIdOf(session);
			if (sessionId === void 0) return;
			host.channel.notify(Method.planState, {
				sessionId,
				active: event.data?.active === true
			});
		}), { global: true });
	}
};

//#endregion
//#region src/extras/approvals.ts
/**
* Approvals for changes.
*
* With its file sandbox open (the only choice inside a container), dsh never asks before running a command or writing a
* file: its approval prompts exist for sandbox escalations. This extra adds the ask. In `ask` mode a call of a changing
* tool (and a command that is not on the read-only list) is put to dsh's approval service, whose answerer in the ACP
* server turns it into the standard permission request. In `bypass` mode nothing asks. Orch chooses the mode of each
* session.
*/
const approvals = {
	id: "approvals",
	async start(host) {
		const service = await host.waitService("approval");
		if (service === void 0) throw new ExtraOff("host_api_missing", "the approval service is not loaded");
		if (typeof service.setPolicy !== "function") throw new ExtraOff("host_api_changed", "the approval service has no setPolicy");
		const { askTools } = host.config.approvals;
		const { commandTool, readCommands } = host.config.planMode;
		const asking = new Set(askTools);
		const modes = /* @__PURE__ */ new Map();
		const modeOf = (sessionId) => modes.get(sessionId) ?? host.config.approvals.mode;
		/** dsh refuses every ask while its policy is `never`, so a session in `ask` mode needs the policy `ask`. */
		const apply = (agent, mode) => {
			service.setPolicy(agent, mode === "ask" ? "ask" : "never");
		};
		host.ctx.on("agent/created", host.guarded("approvals", (payload) => {
			const agent = payload.agent;
			const sessionId = sessionIdOf(agent);
			if (sessionId !== void 0) apply(agent, modeOf(sessionId));
		}), { global: true });
		const fromMeta = (sessionId, meta) => {
			if (typeof meta?.approval_mode !== "string") return;
			const mode = meta.approval_mode === "bypass" ? "bypass" : "ask";
			modes.set(sessionId, mode);
			const agent = host.service("agents")?.get(sessionId);
			if (agent !== void 0) apply(agent, mode);
		};
		host.book.onOpened(host.guarded("approvals", ({ sessionId }) => fromMeta(sessionId, host.book.metaOf(sessionId))));
		host.book.onPrompt(host.guarded("approvals", fromMeta));
		host.ctx.on("tools/pre-execute", (async (call, next) => {
			const refusal = planToolGuard(host).refuse(call);
			if (refusal !== void 0) return {
				kind: "deny",
				reason: refusal
			};
			if (!host.isOn("approvals")) return next();
			const sessionId = sessionIdOf(call.agent);
			if (sessionId === void 0 || modeOf(sessionId) !== "ask") return next();
			if (!asking.has(call.name)) return next();
			if (call.name === commandTool) {
				const given = call.arguments?.command;
				if (typeof given === "string" && isReadOnlyCommand(given, readCommands)) return next();
			}
			return {
				kind: "ask",
				reason: `Run ${call.name}`
			};
		}), {
			global: true,
			prepend: true
		});
		host.channel.handle(Method.approvalSet, (params) => {
			requireOn(host, "approvals");
			const input = paramsOf(params);
			const agent = liveAgent(host, input.sessionId);
			const mode = stringParam(input, "mode");
			if (mode !== "ask" && mode !== "bypass") throw new Error("mode must be 'ask' or 'bypass'");
			modes.set(agent.id, mode);
			apply(agent, mode);
			return { mode };
		});
	}
};

//#endregion
//#region src/executor.ts
/** A failure the executor reported, or one of reaching it. */
var ExecutorError = class extends Error {
	constructor(message) {
		super(message);
		this.name = "ExecutorError";
	}
};
/** How long one call may take before it is given up, in milliseconds: a little over the longest wait a tool makes itself. */
const CALL_TIMEOUT_MS = 7e4;
/** How long one wait for a command's output is, in milliseconds; the executor allows at most this. */
const WAIT_STEP_MS = 25e3;
/** The client. */
var Executor = class {
	address;
	callTimeoutMs;
	nextId = 1;
	/** `callTimeoutMs` is how long a call may take unless the caller says otherwise (a foreground command is let run longer). */
	constructor(address, callTimeoutMs = CALL_TIMEOUT_MS) {
		this.address = address;
		this.callTimeoutMs = callTimeoutMs;
	}
	/**
	* Calls one tool and returns its structured result, else the JSON its text holds, else the text. `timeoutMs` is how long
	* this call may take, for one that the executor itself lets run longer than the usual (a foreground command).
	*/
	async call(tool, args, signal, timeoutMs = this.callTimeoutMs) {
		const id = this.nextId++;
		const body = {
			jsonrpc: "2.0",
			id,
			method: "tools/call",
			params: {
				name: tool,
				arguments: args
			}
		};
		const timeout = AbortSignal.timeout(timeoutMs);
		let response;
		try {
			response = await fetch(this.address.url, {
				method: "POST",
				headers: {
					"content-type": "application/json",
					accept: "application/json, text/event-stream",
					authorization: `Bearer ${this.address.token}`
				},
				body: JSON.stringify(body),
				signal: signal === void 0 ? timeout : AbortSignal.any([signal, timeout])
			});
		} catch (error) {
			throw new ExecutorError(`the executor could not be reached: ${error.message}`);
		}
		const text = await response.text();
		if (!response.ok) throw new ExecutorError(`the executor answered ${response.status}: ${text.slice(0, 300)}`);
		const candidates = (response.headers.get("content-type") ?? "").startsWith("text/event-stream") ? text.split("\n").filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trim()) : [text.trim()];
		for (const candidate of candidates) {
			let message;
			try {
				message = JSON.parse(candidate);
			} catch {
				continue;
			}
			if (message.id !== id) continue;
			if (message.error !== void 0) throw new ExecutorError(message.error.message ?? "the executor refused the call");
			const result = message.result ?? {};
			const said = (Array.isArray(result.content) ? result.content : []).map((block) => block.text ?? "").join("\n");
			if (result.isError === true) throw new ExecutorError(said === "" ? `${tool} failed` : said);
			if (result.structuredContent !== void 0 && result.structuredContent !== null) return result.structuredContent;
			try {
				return JSON.parse(said);
			} catch {
				return said;
			}
		}
		throw new ExecutorError("the executor's answer holds no result");
	}
	/** Reads a text file, or a window of its lines. */
	async read(path, window = {}, signal) {
		const answer = await this.call("read", {
			path,
			...window
		}, signal);
		const text = typeof answer === "string" ? answer : answer?.content ?? answer?.text;
		if (typeof text !== "string") throw new ExecutorError("the executor's answer to a read holds no text");
		return text;
	}
	/** Writes a text file. */
	async write(path, content, signal) {
		await this.call("write", {
			path,
			content
		}, signal);
	}
	/** Starts a command in the background and returns the id of the process. */
	async start(command, cwd, signal) {
		const id = (await this.call("shell", {
			command,
			...cwd === void 0 || cwd === "" ? {} : { cwd },
			background: true
		}, signal))?.process_id;
		if (typeof id !== "string") throw new ExecutorError("the executor started no process");
		return id;
	}
	/** What a process printed since the last read, waiting up to `waitMs` for more; `running` says whether it goes on. */
	async output(processId, waitMs, signal) {
		const answer = await this.call("shell_output", {
			process_id: processId,
			wait_ms: waitMs
		}, signal);
		return {
			output: typeof answer?.output === "string" ? answer.output : "",
			running: answer?.running !== false,
			exitCode: typeof answer?.exit_code === "number" ? answer.exit_code : null
		};
	}
	/** Stops a process. */
	async stop(processId, signal) {
		const answer = await this.call("shell_stop", { process_id: processId }, signal);
		return typeof answer?.exit_code === "number" ? answer.exit_code : null;
	}
	/** Runs a command to its end and returns what it printed and how it ended; a stop of the signal stops the process. */
	async run(command, cwd, signal) {
		const id = await this.start(command, cwd, signal);
		let text = "";
		const stopper = () => void this.stop(id).catch(() => void 0);
		signal?.addEventListener("abort", stopper, { once: true });
		try {
			for (;;) {
				const step = await this.output(id, WAIT_STEP_MS, signal);
				text += step.output;
				if (!step.running) return {
					output: text,
					exitCode: step.exitCode
				};
			}
		} finally {
			signal?.removeEventListener("abort", stopper);
		}
	}
};

//#endregion
//#region src/extras/backends.ts
/**
* Executor backends: the agent's commands and files run in Orch's environment.
*
* In a separated arrangement dsh runs on the provider's machine and the environment is somewhere else. Orch gives the
* session one address and one token (`exec` in `_meta.orch`) of its executor, and this extra registers the tools the
* model uses to reach the environment under the names dsh's own tools have (`bash`, `read`, `write`, `edit`, `glob`,
* `grep`, `job_output`, `job_kill`, `job_list`), so that approvals, plan mode and the model's habits hold unchanged.
*
* Why tools and not dsh's file and shell services: those services are also used by dsh itself (its home, its skills,
* its spill files), by a ripgrep that runs on the harness machine, and by a permission row that refuses to start without
* a sandbox of its own; replacing them would move dsh's own work to the wrong machine. The tools are what the model
* sees, and they alone are re-pointed. `research-backends.md` of the notes has the survey that led here.
*
* The launch has to switch off the rows that register the same names: `tool-bash`, `tool-fs`, `tool-fs-search`,
* `tool-jobs` and `tool-working-directory` (whose directory would be the provider's own). When a name is taken the
* extra goes off and says which.
*/
/** The tool set of the executor this extra speaks; Orch asks for a token of this set. */
const EXECUTOR_TOOL_SET = "plain";
/** The most a command may be told to run for: the executor's own bound. */
const TIMEOUT_MAX_MS = 36e5;
/** How long the executor lets a foreground command run when it is not told. */
const FOREGROUND_DEFAULT_MS = 12e4;
/** How much longer than the command's own timeout a call waits for the executor's answer, in milliseconds. */
const CALL_SLACK_MS = 3e4;
/** How long a `glob` may take before it is stopped; the same as dsh's own. */
const GLOB_TIMEOUT_MS = 3e4;
/** The longest the executor waits for output in one call. */
const OUTPUT_WAIT_MAX_MS = 3e4;
function str(args, key, required = false) {
	const value = args[key];
	if (value === void 0 || value === null) {
		if (required) throw new ExecutorError(`${key} is required`);
		return;
	}
	if (typeof value !== "string") throw new ExecutorError(`${key} must be a string`);
	if (required && value.length === 0) throw new ExecutorError(`${key} must not be empty`);
	return value;
}
function num(args, key) {
	const value = args[key];
	if (value === void 0 || value === null) return void 0;
	if (typeof value !== "number" || !Number.isFinite(value)) throw new ExecutorError(`${key} must be a number`);
	return value;
}
/** Quotes text as one word of a POSIX shell. */
function shellQuote(text) {
	return `'${text.replaceAll("'", `'\\''`)}'`;
}
/**
* The command that lists the files (not directories) a glob matches, newest first, at most `max`. A pattern without a
* `/` matches the base name at any depth, as dsh's own `glob` does; bash's own globbing does the matching, so a
* `**` descends and a leading dot is matched.
*/
function globCommand(pattern, directory, max) {
	const anchored = pattern.includes("/") ? pattern : `**/${pattern}`;
	const body = `shopt -s globstar nullglob dotglob; IFS=; files=(); for f in \$1; do [ -f "\$f" ] && files+=("\$f"); done; [ \${#files[@]} -gt 0 ] && ls -1td -- "\${files[@]}" | head -n ${Math.max(1, Math.floor(max))}; true`;
	return `${directory === void 0 ? "" : `cd ${shellQuote(directory)} && `}bash -c ${shellQuote(body)} _ ${shellQuote(anchored)}`;
}
/** What a finished command is shown as. */
function commandText(done) {
	const parts = [];
	if (typeof done.stdout === "string" && done.stdout.length > 0) parts.push(done.stdout.replace(/\n$/, ""));
	if (typeof done.stderr === "string" && done.stderr.length > 0) parts.push(`[stderr]\n${done.stderr.replace(/\n$/, "")}`);
	if (done.truncated === true) parts.push(done.output_file === void 0 ? "[output was cut in the middle]" : `[output was cut in the middle; the whole of it is in ${done.output_file} in the environment]`);
	if (typeof done.note === "string" && done.note.length > 0) parts.push(`[${done.note}]`);
	if (done.exit_code !== 0) parts.push(done.exit_code === null || done.exit_code === void 0 ? "[the command ended without an exit code]" : `[exit code: ${done.exit_code}]`);
	return parts.length === 0 ? "(no output)" : parts.join("\n");
}
const OUTPUT = {
	type: "object",
	properties: { text: { type: "string" } },
	required: ["text"]
};
function define(name, description, properties, required, execute) {
	return {
		name,
		description,
		parameters: {
			type: "object",
			properties,
			...required.length === 0 ? {} : { required }
		},
		output: {
			schema: OUTPUT,
			render: (_args, value) => [{
				type: "text",
				text: value.text
			}]
		},
		execute: async (args, exec) => ({ text: await execute(args ?? {}, exec) })
	};
}
/** The extra. */
const backends = {
	id: "executor_backends",
	async start(host) {
		const config = host.config.backends;
		if (!config.enabled) throw new ExtraOff("unsupported_environment", "the arrangement is not separated: dsh runs the commands and files itself");
		const tools = await host.waitService("tools");
		if (tools === void 0 || typeof tools.register !== "function") throw new ExtraOff("host_api_missing", "the tools service is not loaded");
		const executors = /* @__PURE__ */ new Map();
		const jobs = /* @__PURE__ */ new Map();
		/** The executor of the session a call runs for: what Orch told of the root session, else the plugin's own setting. */
		const executorOf = (exec) => {
			const root = exec.agent === void 0 ? void 0 : rootSessionId(host, exec.agent.session);
			const told = root === void 0 ? void 0 : host.book.metaOf(root)?.exec;
			let address;
			if (told !== void 0) {
				if (told.tool_set !== void 0 && told.tool_set !== "plain") throw new ExecutorError(`the executor was given for the tool set "${told.tool_set}", and this plugin speaks "${EXECUTOR_TOOL_SET}"`);
				address = {
					url: told.url,
					token: told.token
				};
			} else if (config.url !== null) {
				const token = process.env[config.tokenEnv];
				if (token !== void 0 && token.length > 0) address = {
					url: config.url,
					token
				};
			}
			if (address === void 0) throw new ExecutorError("no executor is named for this session: the commands and files of the environment are out of reach");
			const key = `${address.url}\u0000${address.token}`;
			let found = executors.get(key);
			if (found === void 0) {
				found = new Executor(address);
				executors.set(key, found);
			}
			return found;
		};
		const definitions = [
			define("bash", `Run a shell command in the environment and wait for it. The answer has the standard output, the standard error and the exit code. Each call runs in a fresh shell; pass \`workdir\` instead of using \`cd\`. A command is stopped when \`timeoutMs\` has passed (at most ${TIMEOUT_MAX_MS}); anything longer belongs in the background with \`run_in_background\`, which answers at once with a job id (collect with job_output, stop with job_kill). Long output is cut in the middle. Provide \`description\` before \`command\` in the arguments.`, {
				description: {
					type: "string",
					description: "Clear, concise description of what this command does in active voice, 5-10 words (shown in the UI)."
				},
				command: {
					type: "string",
					description: "The command, as text for the environment's shell."
				},
				timeoutMs: {
					type: "number",
					description: "Timeout in milliseconds; the command is stopped when it has passed. 120000 when left out."
				},
				workdir: {
					type: "string",
					description: "Working directory for this command. Defaults to the session's directory; a relative path is resolved against it."
				},
				run_in_background: {
					type: "boolean",
					description: "Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies."
				}
			}, ["description", "command"], async (args, exec) => {
				const command = str(args, "command", true);
				const workdir = str(args, "workdir");
				const timeout = num(args, "timeoutMs");
				if (timeout !== void 0 && (timeout < 1 || timeout > TIMEOUT_MAX_MS)) throw new ExecutorError(`timeoutMs is from 1 to ${TIMEOUT_MAX_MS}, and ${timeout} is not; put a longer command in the background`);
				const executor = executorOf(exec);
				if (args["run_in_background"] === true) {
					const id = await executor.start(command, workdir, exec.signal);
					jobs.set(id, {
						command,
						running: true,
						exitCode: null
					});
					return `Started in the background as job ${id}. Collect its output with job_output, stop it with job_kill.`;
				}
				const done = await executor.call("shell", {
					command,
					...workdir === void 0 ? {} : { cwd: workdir },
					...timeout === void 0 ? {} : { timeout_ms: Math.floor(timeout) }
				}, exec.signal, (timeout === void 0 ? FOREGROUND_DEFAULT_MS : timeout) + CALL_SLACK_MS);
				return commandText(typeof done === "object" && done !== null ? done : {
					stdout: String(done),
					exit_code: 0
				});
			}),
			define("job_output", "Read a background job: what it printed since the previous read, and its exit code once it has ended.", {
				job_id: {
					type: "string",
					description: "Job id returned by bash with run_in_background."
				},
				wait: {
					type: "boolean",
					description: "Wait for new output or for the end of the job. Defaults to false."
				},
				timeout_ms: {
					type: "number",
					description: "Longest wait in milliseconds with wait: true."
				}
			}, ["job_id"], async (args, exec) => {
				const id = str(args, "job_id", true);
				const wait = args["wait"] === true ? Math.min(num(args, "timeout_ms") ?? config.jobWaitMs, OUTPUT_WAIT_MAX_MS) : 0;
				const step = await executorOf(exec).output(id, Math.max(0, Math.floor(wait)), exec.signal);
				const job = jobs.get(id);
				if (job !== void 0) {
					job.running = step.running;
					job.exitCode = step.exitCode;
				}
				const lines = [step.output.length > 0 ? step.output.replace(/\n$/, "") : "(no new output)"];
				lines.push(step.running ? "[still running]" : `[ended with exit code ${step.exitCode ?? "none"}]`);
				return lines.join("\n");
			}),
			define("job_kill", "Stop a running background job.", {
				job_id: {
					type: "string",
					description: "Job id returned by bash with run_in_background."
				},
				reason: {
					type: "string",
					description: "Optional short reason."
				}
			}, ["job_id"], async (args, exec) => {
				const id = str(args, "job_id", true);
				const code = await executorOf(exec).stop(id, exec.signal);
				const job = jobs.get(id);
				if (job !== void 0) {
					job.running = false;
					job.exitCode = code;
				}
				return `Job ${id} was stopped${code === null ? "" : ` (exit code ${code})`}.`;
			}),
			define("job_list", "List your background jobs of this session with their ids and states.", {}, [], async () => {
				if (jobs.size === 0) return "No background jobs.";
				return [...jobs.entries()].map(([id, job]) => `${id}  ${job.running ? "running" : `ended (exit code ${job.exitCode ?? "none"})`}  ${job.command.length > 80 ? `${job.command.slice(0, 77)}...` : job.command}`).join("\n");
			}),
			define("read", "Read a UTF-8 text file of the environment and return line-numbered content.", {
				file_path: {
					type: "string",
					description: "Path to read: absolute, or relative to the session directory."
				},
				offset: {
					type: "number",
					description: "1-based first line to return. Defaults to 1."
				},
				limit: {
					type: "number",
					description: "Maximum number of lines to return. Defaults to 2000."
				}
			}, ["file_path"], async (args, exec) => {
				const offset = num(args, "offset");
				const limit = num(args, "limit");
				const text = await executorOf(exec).read(str(args, "file_path", true), {
					...offset === void 0 ? {} : { offset: Math.floor(offset) },
					...limit === void 0 ? {} : { limit: Math.floor(limit) }
				}, exec.signal);
				return text === "" ? "(nothing to show: the file has no text, or it is an image, which this tool does not show)" : text;
			}),
			define("write", "Write a file of the environment with the given content, replacing it if it exists. Parent directories are created.", {
				file_path: {
					type: "string",
					description: "Path to write: absolute, or relative to the session directory."
				},
				content: {
					type: "string",
					description: "The whole content of the file."
				}
			}, ["file_path", "content"], async (args, exec) => {
				const target = str(args, "file_path", true);
				const content = args["content"];
				if (typeof content !== "string") throw new ExecutorError("content must be a string");
				await executorOf(exec).write(target, content, exec.signal);
				return `Wrote ${Buffer.byteLength(content, "utf8")} bytes to ${target}.`;
			}),
			define("edit", "Edit an existing UTF-8 text file of the environment by replacing literal text.", {
				file_path: {
					type: "string",
					description: "Path to edit. Provide `file_path` before `old_string` and `new_string` in the arguments."
				},
				old_string: {
					type: "string",
					description: "Literal text to replace."
				},
				new_string: {
					type: "string",
					description: "Literal replacement text. Use an empty string to delete the match."
				},
				replace_all: {
					type: "boolean",
					description: "Replace all matches. Defaults to false; when false, old_string must appear exactly once."
				}
			}, [
				"file_path",
				"old_string",
				"new_string"
			], async (args, exec) => {
				const target = str(args, "file_path", true);
				const oldText = str(args, "old_string", true);
				const newText = args["new_string"];
				if (typeof newText !== "string") throw new ExecutorError("new_string must be a string");
				const answer = await executorOf(exec).call("edit", {
					path: target,
					old: oldText,
					new: newText,
					all: args["replace_all"] === true
				}, exec.signal);
				const replaced = typeof answer === "object" && answer !== null && typeof answer.replaced === "number" ? answer.replaced : void 0;
				return replaced === void 0 ? `Edited ${target}.` : `Replaced ${replaced} occurrence${replaced === 1 ? "" : "s"} in ${target}.`;
			}),
			define("glob", `Find files, not directories, of the environment whose paths match a glob pattern, including hidden files. Returns up to ${config.globMax} paths, newest first.`, {
				pattern: {
					type: "string",
					description: "Glob pattern to match file paths against (e.g. \"**/*.ts\", \"src/**/*.test.js\"). A pattern with no \"/\" matches the base name at any depth; include a separator to anchor the depth."
				},
				path: {
					type: "string",
					description: "Directory to search in. Defaults to the session's directory; a relative path resolves against it."
				}
			}, ["pattern"], async (args, exec) => {
				const pattern = str(args, "pattern", true);
				const done = await executorOf(exec).call("shell", {
					command: globCommand(pattern, str(args, "path"), config.globMax),
					timeout_ms: GLOB_TIMEOUT_MS
				}, exec.signal);
				if (typeof done?.exit_code === "number" && done.exit_code !== 0) throw new ExecutorError(commandText(done));
				const text = typeof done?.stdout === "string" ? done.stdout.trim() : "";
				return text.length === 0 ? "No files match." : text;
			}),
			define("grep", "Search file contents of the environment with a regular expression. Returns matching lines as `path:line: text`.", {
				pattern: {
					type: "string",
					description: "Regular expression to search for."
				},
				path: {
					type: "string",
					description: "File or directory to search. Defaults to the session's directory; a relative path resolves against it."
				},
				include: {
					type: "string",
					description: "One glob filter for which files to search (e.g. \"*.ts\")."
				}
			}, ["pattern"], async (args, exec) => {
				const path = str(args, "path");
				const include = str(args, "include");
				const answer = await executorOf(exec).call("search", {
					pattern: str(args, "pattern", true),
					...path === void 0 ? {} : { path },
					...include === void 0 ? {} : { glob: include },
					max_results: Math.max(1, Math.floor(config.grepMax))
				}, exec.signal);
				const text = typeof answer === "string" ? answer : JSON.stringify(answer);
				return text.length === 0 ? "No matches." : text;
			})
		];
		const disposers = [];
		try {
			for (const definition of definitions) disposers.push(tools.register(definition));
		} catch (error) {
			for (const dispose of disposers.splice(0)) dispose();
			const reason = error.message;
			throw new ExtraOff("self_check_failed", `${reason}; the launch has to switch off tool-bash, tool-fs, tool-fs-search, tool-jobs and tool-working-directory`);
		}
		host.log(`executor backends: ${definitions.map((d) => d.name).join(", ")} reach the environment`);
	}
};

//#endregion
//#region src/extras/compaction.ts
/**
* Compaction phases, and a manual run.
*
* dsh compacts long histories on its own: it first cuts old tool output, then replaces older turns with a summary,
* and logs each step. ACP shows none of it. This extra reads the log as it grows and reports the phases, and runs a
* compaction when Orch asks for one.
*/
/** How long cut tool output is gathered into one `pruned` notice, in milliseconds; a prune cuts many results at once. */
const PRUNE_GATHER_MS = 100;
const compaction = {
	id: "compaction",
	async start(host) {
		const service = await host.waitService("compaction");
		if (service === void 0) throw new ExtraOff("host_api_missing", "the compaction service is not loaded");
		if (typeof service.compactNow !== "function") throw new ExtraOff("host_api_changed", "the compaction service has no compactNow");
		const runs = /* @__PURE__ */ new Map();
		const send = (notice) => host.channel.notify(Method.compaction, notice);
		const flushPruned = (sessionId, run) => {
			if (run.timer !== void 0) clearTimeout(run.timer);
			run.timer = void 0;
			if (run.pruned === 0) return;
			send({
				sessionId,
				turn: run.turn,
				phase: "pruned",
				replacedTokens: run.pruned
			});
			run.pruned = 0;
		};
		host.ctx.on("session/event", host.guarded("compaction", (session, event) => {
			if (!event.type.startsWith("compaction/")) return;
			const sessionId = sessionIdOf(session);
			if (sessionId === void 0) return;
			if (event.type === "compaction/start") {
				const turn = typeof event.data?.turn === "number" ? event.data.turn : null;
				runs.set(sessionId, {
					turn,
					pruned: 0,
					timer: void 0
				});
				send({
					sessionId,
					turn,
					phase: "started"
				});
				return;
			}
			const run = runs.get(sessionId) ?? {
				turn: null,
				pruned: 0,
				timer: void 0
			};
			if (event.type === "compaction/prune") {
				run.pruned += typeof event.data?.shadowedTokenCount === "number" ? event.data.shadowedTokenCount : 0;
				runs.set(sessionId, run);
				run.timer ??= setTimeout(() => flushPruned(sessionId, run), PRUNE_GATHER_MS);
			} else if (event.type === "compaction/summary") {
				flushPruned(sessionId, run);
				const notice = {
					sessionId,
					turn: run.turn,
					phase: "summarized"
				};
				if (typeof event.data?.shadowedTokenCount === "number") notice.replacedTokens = event.data.shadowedTokenCount;
				send(notice);
			} else if (event.type === "compaction/end") {
				flushPruned(sessionId, run);
				const error = event.data?.error;
				const notice = {
					sessionId,
					turn: run.turn,
					phase: error === void 0 ? "completed" : "failed"
				};
				if (error !== void 0) notice.error = typeof error === "string" ? error : JSON.stringify(error);
				send(notice);
				runs.delete(sessionId);
			}
		}), { global: true });
		host.channel.handle(Method.compactionRun, async (params) => {
			requireOn(host, "compaction");
			const agent = liveAgent(host, paramsOf(params)["sessionId"]);
			try {
				const result = await service.compactNow(agent, new AbortController().signal);
				return { compacted: result !== null && result !== void 0 };
			} catch (error) {
				const code = error.code;
				if (error.name === "ManualCompactionError") throw new RpcFault(ErrorCode.notNow, `compaction did not run: ${String(code ?? error.message)}`);
				throw error;
			}
		});
	}
};

//#endregion
//#region src/extras/records.ts
/**
* Writing a session into dsh's store without running it: fork and history carry both make a new session from a list of
* events. The agent lifecycle is what attaches dsh's session writer, so the session is created as an agent with that
* history as its seed, flushed to disk, and let go; `session/resume` then opens it like any other stored session.
*/
/**
* The route a new session records when its caller names none: the one dsh is configured with (its default agent model),
* or else that of a live agent of this process. dsh refuses to open a session that records a route it does not have.
*/
function defaultRoute(host) {
	const configured = host.service("agentDefaultModel");
	const given = configured?.config ?? configured;
	if (typeof given?.provider === "string" && typeof given.model === "string") return {
		provider: given.provider,
		model: given.model
	};
	const agents = host.service("agents");
	for (const agent of agents?.list?.() ?? []) {
		const options = agent.options;
		if (typeof options?.provider === "string" && typeof options.model === "string") return {
			provider: options.provider,
			model: options.model
		};
	}
}
/** Creates the session in dsh's store and returns when it is on disk. */
async function storeSession(host, spec) {
	const agents = host.service("agents");
	const sessions = host.service("sessions");
	if (agents === void 0 || sessions === void 0) throw new RpcFault(ErrorCode.notNow, "dsh has no agent registry in this process");
	const meta = { cwd: spec.cwd };
	if (spec.parentSession !== void 0) {
		meta.parentSession = spec.parentSession;
		meta.isSeeded = true;
	}
	const options = {
		sessionId: spec.sessionId,
		meta,
		seed: spec.seed
	};
	if (spec.inheritedEventCount !== void 0) options.inheritedEventCount = spec.inheritedEventCount;
	if (spec.provider !== void 0 && spec.model !== void 0) options.agentOptions = {
		provider: spec.provider,
		model: spec.model
	};
	const handle = await agents.create(options);
	try {
		await sessions.flush(handle.agent.session);
	} finally {
		await handle.dispose();
	}
	let reopened;
	try {
		const reopen = { resumeSessionId: spec.sessionId };
		if (options.agentOptions !== void 0) reopen.agentOptions = options.agentOptions;
		reopened = await agents.resume(reopen);
	} catch (error) {
		throw new RpcFault(ErrorCode.internalError, `the new session cannot be opened again: ${causeText(error)}`);
	} finally {
		await reopened?.dispose();
	}
}
/** The message of an error and of the errors that caused it. */
function causeText(error) {
	const parts = [];
	for (let at = error; at instanceof Error && parts.length < 5; at = at.cause) parts.push(at.message);
	return parts.join(": ");
}
/** The events of a session, copied so that the copy may be stored. */
function copyEvents(events) {
	return structuredClone(events);
}

//#endregion
//#region src/extras/fork.ts
/**
* Fork: a copy of a session up to the end of a turn.
*
* ACP has no fork, and the one dsh has is for its own sub-agents. The copy is written as a stored session whose
* header names the source as its parent and the number of events it inherited; Orch then resumes it with the ordinary
* `session/resume`.
*/
const fork = {
	id: "fork",
	async start(host) {
		if (await host.waitService("agents") === void 0) throw new ExtraOff("host_api_missing", "the agent registry is not loaded");
		if (await host.waitService("sessions") === void 0) throw new ExtraOff("host_api_missing", "the session store is not loaded");
		host.channel.handle(Method.sessionFork, async (params) => {
			requireOn(host, "fork");
			const input = paramsOf(params);
			const agent = liveAgent(host, input["sessionId"]);
			const through = input["throughTurn"];
			if (through !== void 0 && (typeof through !== "number" || !Number.isInteger(through) || through < 1)) throw new RpcFault(ErrorCode.invalidParams, "throughTurn must be a whole number of 1 or more");
			const events = agent.session.snapshotEvents();
			const ends = events.filter((event) => event.type === "turn/end");
			const boundary = through === void 0 ? ends[ends.length - 1] : ends.find((event) => event.data?.turn === through);
			if (boundary === void 0) throw new RpcFault(ErrorCode.notNow, through === void 0 ? "the session has no finished turn to copy" : `turn ${through} has not finished`);
			const prefix = copyEvents(events.slice(0, boundary.seq + 1));
			const childId = randomUUID();
			const cwd = agent.session.header.cwd;
			if (cwd === void 0) throw new RpcFault(ErrorCode.notNow, "the session has no working directory");
			const options = agent.options;
			const spec = {
				sessionId: childId,
				cwd,
				seed: prefix
			};
			if (options?.provider !== void 0) spec.provider = options.provider;
			if (options?.model !== void 0) spec.model = options.model;
			await storeSession(host, spec);
			return {
				sessionId: childId,
				turns: ends.filter((event) => event.seq <= boundary.seq).length
			};
		});
	}
};

//#endregion
//#region src/neutral.ts
/** The most characters of one text of a step that is written out. */
const STEP_TEXT_MAX_CHARS = 2e3;
/** The most characters of a difference that is written out. */
const STEP_DIFF_MAX_CHARS = 4e3;
/** How many matches of a file search are written out. */
const STEP_MATCHES_SHOWN = 20;
/** How many results of a web search are written out. */
const STEP_RESULTS_SHOWN = 10;
/** Tracks what became of each item, for the report. */
var Tally = class {
	kinds = /* @__PURE__ */ new Map();
	reasons = /* @__PURE__ */ new Map();
	warnings = [];
	entry(kind) {
		let found = this.kinds.get(kind);
		if (found === void 0) {
			found = {
				kept: 0,
				converted: 0,
				dropped: 0
			};
			this.kinds.set(kind, found);
		}
		return found;
	}
	keep(kind) {
		this.entry(kind).kept += 1;
	}
	convert(kind) {
		this.entry(kind).converted += 1;
	}
	drop(kind, reason) {
		this.entry(kind).dropped += 1;
		const key = `${kind}\u0000${reason}`;
		this.reasons.set(key, (this.reasons.get(key) ?? 0) + 1);
	}
	report() {
		const byKind = {};
		let itemsTotal = 0;
		for (const [kind, counts] of this.kinds) {
			byKind[kind] = { ...counts };
			itemsTotal += counts.kept + counts.converted + counts.dropped;
		}
		const dropped = [...this.reasons].map(([key, count]) => {
			const [kind = "", reason = ""] = key.split("\0");
			return {
				kind,
				count,
				reason
			};
		});
		return {
			fidelity: "structured",
			itemsTotal,
			byKind,
			dropped,
			warnings: this.warnings
		};
	}
};
function clipStart(text, most) {
	const chars = [...text];
	return chars.length <= most ? [text, false] : [chars.slice(0, most).join(""), true];
}
function clipEnd(text, most) {
	const chars = [...text];
	return chars.length <= most ? [text, false] : [chars.slice(chars.length - most).join(""), true];
}
function fenced(language, body) {
	return `\`\`\`${language}\n${body.replace(/\n+$/, "")}\n\`\`\``;
}
function indent(block) {
	return block.split("\n").map((line) => `  ${line}`).join("\n");
}
/** How a step ended, in the words of the written form: ` · exit 1`, ` · failed`, ` · still running`. */
function ending(item) {
	const code = item["payload"]?.tool?.exit_code;
	if (typeof code === "number") return ` · exit ${code}`;
	if (item["status"] === "failed") return " · failed";
	if (item["status"] === "in_progress") return " · still running";
	return "";
}
function stepText(item) {
	const payload = item["payload"];
	if (payload === void 0 || payload === null) return void 0;
	let text = `- ${typeof payload.title === "string" ? payload.title : "A step"}${ending(item)}`;
	const tool = payload.tool ?? {};
	const detail = [];
	const leftOut = (what) => {
		detail.push(`(${what} was left out here)`);
	};
	switch (tool.kind) {
		case "command": {
			if (typeof tool.cwd === "string" && tool.cwd !== "") detail.push(`Directory: ${tool.cwd}`);
			const output = tool.output?.inline;
			if (typeof output === "string" && output.trim() !== "") {
				const [shown, cut] = clipEnd(output, STEP_TEXT_MAX_CHARS);
				detail.push(fenced("", shown));
				if (cut) leftOut("the beginning of the output");
			}
			break;
		}
		case "file_change":
			for (const change of Array.isArray(tool.changes) ? tool.changes : []) if (typeof change?.diff === "string" && change.diff !== "") {
				const [shown, cut] = clipStart(change.diff, STEP_DIFF_MAX_CHARS);
				detail.push(fenced("diff", shown));
				if (cut) leftOut("the rest of the difference");
			}
			break;
		case "file_search":
			detail.push(`Looked for ${JSON.stringify(tool.query ?? "")}${typeof tool.path === "string" ? ` in ${tool.path}` : ""}`);
			for (const hit of (Array.isArray(tool.matches) ? tool.matches : []).slice(0, 20)) detail.push(String(hit?.path ?? ""));
			break;
		case "web_search":
			detail.push(`Searched for ${JSON.stringify(tool.query ?? "")}`);
			for (const result of (Array.isArray(tool.results) ? tool.results : []).slice(0, 10)) detail.push(`${result?.title ?? ""} ${result?.url ?? ""}`);
			break;
		case "mcp":
		case "orch":
		case "other":
			for (const [label, member] of [["Arguments", "arguments"], ["Result", "result"]]) {
				const value = tool[member];
				if (value !== void 0 && value !== null) {
					const [shown, cut] = clipStart(JSON.stringify(value), STEP_TEXT_MAX_CHARS);
					detail.push(`${label}: ${shown}`);
					if (cut) leftOut("the rest of it");
				}
			}
			break;
		case "subagent": if (typeof tool.prompt === "string") detail.push(`Asked: ${clipStart(tool.prompt, STEP_TEXT_MAX_CHARS)[0]}`);
	}
	if (detail.length > 0) text += `\n${indent(detail.join("\n"))}`;
	return text;
}
function planText(item) {
	const steps = item["payload"]?.steps;
	if (!Array.isArray(steps) || steps.length === 0) return void 0;
	const lines = ["Plan:"];
	for (const step of steps) {
		const mark = step?.status === "completed" ? "[x]" : step?.status === "in_progress" ? "[~]" : "[ ]";
		lines.push(`- ${mark} ${step?.text ?? ""}`);
	}
	return lines.join("\n");
}
function userText(item) {
	const parts = item["payload"]?.parts;
	if (!Array.isArray(parts)) return "";
	const out = [];
	for (const part of parts) switch (part?.type) {
		case "text":
			if (typeof part.text === "string") out.push(part.text);
			break;
		case "image":
			out.push("[image omitted]");
			break;
		case "file":
			out.push(`Attached file: ${part.path ?? ""}`);
			break;
		case "forwarded": if (typeof part.text === "string") out.push(`[Forwarded]\n${part.text}`);
	}
	return out.join("\n\n");
}
/** Groups the main-stream items into turns, by the turn each item names; the report counts what is left behind. */
function turnsOf(lines, tally) {
	const turns = [];
	for (const line of lines) {
		if (line.trim() === "") continue;
		let item;
		try {
			item = JSON.parse(line);
		} catch {
			throw new Error("a line of the history is not JSON");
		}
		const kind = typeof item["kind"] === "string" ? item["kind"] : "item";
		if ((item["stream"] ?? "main") !== "main") {
			tally.drop(kind, "it belongs to a sub-agent's own stream");
			continue;
		}
		const id = typeof item["turn_id"] === "string" ? item["turn_id"] : typeof item["turn_ordinal"] === "number" ? String(item["turn_ordinal"]) : null;
		const last = turns[turns.length - 1];
		if (last !== void 0 && last.id === id) last.items.push(item);
		else turns.push({
			id,
			items: [item]
		});
	}
	return turns;
}
/** One turn's assistant text: what the agent said, the steps in words, the plan, and the final answer last. */
function assistantText(turn, tally) {
	let user;
	const said = [];
	const steps = [];
	let plan;
	for (const item of turn.items) {
		const kind = typeof item["kind"] === "string" ? item["kind"] : "item";
		if (kind === "user_message") {
			const text = userText(item);
			if (text === "") tally.drop(kind, "it carried nothing the model can be told");
			else {
				tally.keep(kind);
				user = user === void 0 ? text : `${user}\n\n${text}`;
			}
		} else if (kind === "agent_message") {
			const text = item["payload"]?.text;
			if (typeof text !== "string" || text === "") tally.drop(kind, "it said nothing");
			else {
				tally.keep(kind);
				said.push(text);
			}
		} else if (kind === "tool_call") {
			const written = stepText(item);
			if (written !== void 0) {
				tally.convert(kind);
				steps.push(written);
			}
		} else if (kind === "plan") {
			const written = planText(item);
			if (written === void 0) tally.drop(kind, "it had no steps");
			else {
				tally.convert(kind);
				plan = written;
			}
		} else tally.drop(kind, "the model is not told of it");
	}
	const final = said.pop();
	const parts = [...said];
	const work = [];
	if (steps.length > 0) work.push(`Steps of this turn:\n${steps.join("\n")}`);
	if (plan !== void 0) work.push(plan);
	if (work.length > 0) parts.push(work.join("\n\n"));
	if (final !== void 0) parts.push(final);
	return {
		user,
		assistant: parts.length > 0 ? parts.join("\n\n") : void 0
	};
}
/**
* Converts a neutral history into the events of a new session.
* @param text - the history file's text
* @param options - the route recorded on the messages, ids and times
* @returns the events, contiguous from seq 0, the number of turns carried, and what became of each item
*/
function neutralToSeed(text, options) {
	const tally = new Tally();
	let turns = turnsOf(text.split("\n"), tally);
	if (options.throughTurn !== void 0) turns = turns.slice(0, options.throughTurn);
	const events = [];
	let time = options.startTime;
	const push = (type, data, surface = false) => {
		const event = {
			type,
			seq: events.length,
			time: time++,
			data
		};
		if (surface) event.surfaceOp = "append";
		events.push(event);
		return event;
	};
	let carried = 0;
	let headerWritten = false;
	for (const turn of turns) {
		const { user, assistant } = assistantText(turn, tally);
		if (user === void 0 && assistant === void 0) continue;
		carried += 1;
		push("turn/start", { turn: carried });
		push("step/start", {
			turn: carried,
			step: 1
		});
		if (user !== void 0) push("user/message", {
			content: [{
				type: "text",
				text: user
			}],
			source: { kind: "user" },
			role: "user",
			id: options.newId()
		}, true);
		if (assistant !== void 0) {
			if (!headerWritten) {
				push("request/header", {
					header: { config: {
						provider: options.provider,
						model: options.model
					} },
					reason: "initial",
					startsSeries: true
				});
				push("request/context", {
					provider: options.provider,
					model: options.model
				});
				headerWritten = true;
			}
			const at = time;
			push("assistant/message", {
				turn: carried,
				step: 1,
				message: {
					role: "assistant",
					content: [{
						type: "text",
						text: assistant
					}],
					source: {
						kind: "model",
						provider: options.provider,
						model: options.model
					},
					id: options.newId()
				},
				stream: [
					{
						type: "chunk",
						time: at,
						chunk: {
							type: "block-start",
							index: 0,
							blockType: "text"
						}
					},
					{
						type: "text-chunks",
						time0: at,
						index: 0,
						dt: [],
						texts: [assistant]
					},
					{
						type: "chunk",
						time: at,
						chunk: {
							type: "block-end",
							index: 0,
							block: {
								type: "text",
								text: assistant
							}
						}
					},
					{
						type: "chunk",
						time: at,
						chunk: {
							type: "finish",
							reason: { kind: "stop" }
						}
					}
				]
			}, true);
		}
		push("step/end", {
			turn: carried,
			step: 1
		});
		push("turn/end", {
			turn: carried,
			reason: { kind: "completed" }
		});
	}
	return {
		events,
		turns: carried,
		report: tally.report()
	};
}

//#endregion
//#region src/extras/history.ts
/**
* History carry: Orch's neutral history written into a new dsh session, and a dsh session's own record written out.
*
* A thread that is switched to dsh brings its history with it. Orch writes the history as a file of neutral items; the
* new session is made from it in dsh's own format (version 4), so that dsh resumes it as it resumes any session of
* its own, with the conversation in its log and not as a pasted notice. Export copies the stored record of a session
* so that another dsh of the same format can take it over.
*/
/** The stored file of a session: `<sessions>/--<cwd>--/<id>/session.v4.jsonl[.zstd]`. */
async function storedFile(sessionsRoot, sessionId) {
	for (const bucket of await readdir(sessionsRoot)) {
		const dir = path.join(sessionsRoot, bucket, sessionId);
		try {
			for (const name of await readdir(dir)) if (/^session\.v4\.jsonl(\.zstd)?$/.test(name)) return path.join(dir, name);
		} catch {}
	}
}
const history = {
	id: "native_carry",
	async start(host) {
		if (await host.waitService("agents") === void 0) throw new ExtraOff("host_api_missing", "the agent registry is not loaded");
		if (await host.waitService("sessions") === void 0) throw new ExtraOff("host_api_missing", "the session store is not loaded");
		host.channel.handle(Method.historyImport, async (params) => {
			requireOn(host, "native_carry");
			const input = paramsOf(params);
			if (input["format"] !== "orch-neutral-jsonl-v1") throw new RpcFault(ErrorCode.invalidParams, `format must be ${NEUTRAL_FORMAT}`);
			const file = stringParam(input, "path");
			const cwd = stringParam(input, "cwd");
			if (!path.isAbsolute(cwd)) throw new RpcFault(ErrorCode.invalidParams, "cwd must be an absolute path");
			const through = input["throughTurn"];
			if (through !== void 0 && through !== null && (typeof through !== "number" || !Number.isInteger(through) || through < 0)) throw new RpcFault(ErrorCode.invalidParams, "throughTurn must be a whole number");
			let text;
			try {
				text = await readFile(file, "utf8");
			} catch (error) {
				throw new RpcFault(ErrorCode.invalidParams, `the history file cannot be read: ${error.message}`);
			}
			const route = (typeof input["provider"] === "string" && typeof input["model"] === "string" ? {
				provider: input["provider"],
				model: input["model"]
			} : void 0) ?? defaultRoute(host);
			if (route === void 0) throw new RpcFault(ErrorCode.invalidParams, "provider and model must be given: no session is open to take them from");
			const sessionId = randomUUID();
			let converted;
			try {
				converted = neutralToSeed(text, {
					...route,
					startTime: Date.now(),
					newId: randomUUID,
					...typeof through === "number" ? { throughTurn: through } : {}
				});
			} catch (error) {
				throw new RpcFault(ErrorCode.invalidParams, error.message);
			}
			await storeSession(host, {
				sessionId,
				cwd,
				seed: converted.events,
				...route
			});
			return {
				sessionId,
				report: converted.report
			};
		});
		host.channel.handle(Method.historyExport, async (params) => {
			requireOn(host, "native_carry");
			const input = paramsOf(params);
			if (input["format"] !== "dsh-session-v4") throw new RpcFault(ErrorCode.invalidParams, `format must be ${NATIVE_FORMAT}`);
			const agent = liveAgent(host, input["sessionId"]);
			const target = stringParam(input, "path");
			const home = host.service("dshHomePath");
			if (typeof home !== "function") throw new RpcFault(ErrorCode.notNow, "the home of dsh is not known to this process");
			await host.service("sessions")?.flush(agent.session);
			const source = await storedFile(home("sessions"), agent.id);
			if (source === void 0) throw new RpcFault(ErrorCode.notNow, "the session has no stored record yet");
			await copyFile(source, target);
			return {
				format: NATIVE_FORMAT,
				path: target,
				size: (await stat(target)).size
			};
		});
	}
};

//#endregion
//#region src/extras/tool-calls.ts
/** The book of this process; installs its listeners on first use. */
function toolCallBook(host) {
	return host.shared("tool-calls", () => {
		const calls = /* @__PURE__ */ new Map();
		const key = (sessionId, tool) => `${sessionId}\u0000${tool}`;
		host.ctx.on("tools/pre-execute", (async (call, next) => {
			const sessionId = sessionIdOf(call.agent);
			if (sessionId !== void 0) calls.set(key(sessionId, call.name), call.callId);
			return next();
		}), { global: true });
		return { running: (sessionId, tool) => calls.get(key(sessionId, tool)) };
	});
}

//#endregion
//#region src/extras/questions.ts
/**
* Questions to people.
*
* dsh's `ask_user_question` tool, and its plan review, ask `ctx.userQuestions`, which hands the question to the first
* listener of the `user-questions/request` waterfall that answers. dsh's ACP server has none, so the call fails with
* "no provider". This extra answers it: it sends the questions to Orch as a request and waits as long as the person
* takes, and turns the plan review into `_orch/plan/review`.
*/
/** Makes an error of dsh's own question class, so that its tools treat it as dsh does; a plain error if it cannot. */
async function questionError(host, code, message) {
	const service = host.service("userQuestions");
	let ErrorClass;
	try {
		await service?.ask({ questions: [] });
	} catch (error) {
		const ctor = error?.constructor;
		if (typeof ctor === "function" && error.name === "UserQuestionError") ErrorClass = ctor;
	}
	return ErrorClass !== void 0 ? new ErrorClass(message, code) : Object.assign(new Error(message), {
		name: "UserQuestionError",
		code
	});
}
function questionOf(item) {
	const question = {
		id: item.id,
		question: item.question
	};
	if (item.detail !== void 0) question.detail = item.detail;
	if (item.header !== void 0) question.header = item.header;
	if (item.options !== void 0) question.options = item.options.map((o) => o.description === void 0 ? { label: o.label } : {
		label: o.label,
		description: o.description
	});
	if (item.multiSelect === true) question.multiSelect = true;
	return question;
}
const questions = {
	id: "questions",
	async start(host) {
		if (await host.waitService("userQuestions") === void 0) throw new ExtraOff("host_api_missing", "the user-questions service is not loaded");
		const tools = host.service("tools");
		const deadline = Date.now() + host.config.serviceWaitMs;
		while (tools?.get("ask_user_question") === void 0) {
			if (Date.now() > deadline) throw new ExtraOff("host_api_missing", "the ask_user_question tool is not loaded: add the row of @deepseek-ai/dsh-tool-ask-user to the profile");
			await new Promise((resolve) => setTimeout(resolve, 50));
		}
		const calls = toolCallBook(host);
		const answer = async (request, next) => {
			if (!host.isOn("questions") || host.channel.isClosed) return next();
			const sessionId = sessionIdOf(request.agent);
			if (sessionId === void 0 || request.questions.length === 0) return next();
			const review = request.questions.find((q) => q.intent?.kind === "plan-review");
			const callId = request.wait?.callId ?? review?.intent?.callId ?? calls.running(sessionId, "ask_user_question") ?? randomUUID();
			const closed = (reason) => host.channel.notify(Method.questionClosed, {
				sessionId,
				callId,
				reason
			});
			const withdraw = new AbortController();
			const onAbort = () => {
				closed("cancelled");
				withdraw.abort();
			};
			request.signal?.addEventListener("abort", onAbort, { once: true });
			try {
				if (review !== void 0) return await planReview(host, request, review, sessionId, callId, withdraw.signal);
				return { answers: completeAnswers(request, await host.channel.request(Method.questionAsk, {
					sessionId,
					callId,
					questions: request.questions.map(questionOf)
				}, { signal: withdraw.signal })) };
			} catch (error) {
				if (error instanceof RpcFault && request.signal?.aborted !== true) {
					if (error.code === ErrorCode.declined) throw await questionError(host, "ASK_CANCELLED", "The person did not answer.");
					if (error.code === ErrorCode.methodNotFound) return next();
				}
				throw error;
			} finally {
				request.signal?.removeEventListener("abort", onAbort);
			}
		};
		host.ctx.on("user-questions/request", (async (request, next) => {
			try {
				return await answer(request, next);
			} catch (error) {
				if (error instanceof RpcFault) throw await questionError(host, "ASK_FAILED", error.message);
				throw error;
			}
		}), {
			global: true,
			prepend: true
		});
	}
};
/** One answer for every question, in question order: an unanswered one is skipped, never invented. */
function completeAnswers(request, result) {
	const given = new Map((result?.answers ?? []).map((a) => [a.id, a]));
	return request.questions.map((q) => {
		const a = given.get(q.id);
		if (a === void 0) return {
			id: q.id,
			selected: []
		};
		const out = {
			id: q.id,
			selected: Array.isArray(a.selected) ? a.selected.map(String) : []
		};
		if (typeof a.custom === "string" && a.custom.length > 0) out.custom = a.custom;
		return out;
	});
}
/** Sends the plan for review and turns the decision into the answer dsh's exit tool reads. */
async function planReview(host, request, review, sessionId, callId, signal) {
	const approve = review.intent?.approve ?? review.options?.[0]?.label ?? "Approve";
	const keep = review.options?.find((o) => o.label !== approve)?.label ?? "Keep planning";
	const result = await host.channel.request(Method.planReview, {
		sessionId,
		callId,
		plan: review.detail ?? review.question
	}, { signal });
	const decision = result?.decision ?? "dismiss";
	if (decision === "proceed") return { answers: [{
		id: review.id,
		selected: [approve]
	}] };
	if (decision === "keep_planning") {
		const feedback = typeof result.feedback === "string" ? result.feedback.trim() : "";
		return { answers: [{
			id: review.id,
			selected: feedback === "" ? [keep] : [],
			...feedback === "" ? {} : { custom: feedback }
		}] };
	}
	throw await questionError(host, "ASK_CANCELLED", "The plan review was dismissed.");
}

//#endregion
//#region src/extras/skills.ts
/**
* Skills of the session.
*
* dsh reads skills from `skills/<name>/SKILL.md` under its home, among other places, and its `skill` tool loads them.
* Orch lays out each session's skills in a directory it names in `_meta.orch.skills_dir`. Where that directory is one
* dsh reads already (its own home), nothing more is done; where it is not, the plugin registers it as one more source
* of skills, with the folder layout and front matter dsh's own provider reads, so that the catalogue shown to the
* model and the `skill` tool cover it. The extra also answers `_orch/skills/refresh` with the names a session sees
* now, which is how Orch confirms that a skill it just wrote is visible.
*/
/** Reads `---` front matter: single-line values and the folded and literal block forms. */
function parseSkill(text) {
	const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(text);
	if (match === null) return void 0;
	const values = /* @__PURE__ */ new Map();
	const lines = (match[1] ?? "").split(/\r?\n/);
	for (let at = 0; at < lines.length; at += 1) {
		const found = /^([A-Za-z][\w-]*):\s*(.*)$/.exec(lines[at] ?? "");
		if (found === null) continue;
		const key = found[1] ?? "";
		let value = (found[2] ?? "").trim();
		if (/^[>|][+-]?$/.test(value)) {
			const folded = value.startsWith(">");
			const block = [];
			while (at + 1 < lines.length && /^\s+\S|^\s*$/.test(lines[at + 1] ?? "")) {
				at += 1;
				block.push((lines[at] ?? "").trim());
			}
			value = (folded ? block.join(" ") : block.join("\n")).trim();
		} else if (value.startsWith("\"") && value.endsWith("\"") || value.startsWith("'") && value.endsWith("'")) value = value.slice(1, -1);
		values.set(key, value);
	}
	const name = values.get("name");
	const description = values.get("description");
	if (name === void 0 || description === void 0 || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) return void 0;
	const skill = {
		name,
		description,
		modelInvocable: values.get("disable-model-invocation") !== "true",
		userInvocable: values.get("user-invocable") !== "false",
		body: (match[2] ?? "").trim()
	};
	const whenToUse = values.get("when_to_use") ?? values.get("when-to-use");
	if (whenToUse !== void 0) skill.whenToUse = whenToUse;
	return skill;
}
async function realOrSame(target) {
	try {
		return await realpath(target);
	} catch {
		return path.resolve(target);
	}
}
/** The rank of the plugin's source among dsh's own: with the custom directories. */
const CUSTOM_RANK = 300;
const skills = {
	id: "skills",
	async start(host) {
		const registry = await host.waitService("skills");
		if (registry === void 0) throw new ExtraOff("host_api_missing", "the skills registry is not loaded");
		if (typeof registry.list !== "function") throw new ExtraOff("host_api_changed", "the skills registry has no list");
		const directories = /* @__PURE__ */ new Set();
		let invalidate;
		if (typeof registry.registerProvider === "function") {
			registry.registerProvider((control) => {
				invalidate = control.invalidate;
				return {
					name: "orch-skills",
					async list() {
						const found = [];
						for (const dir of directories) {
							let entries = [];
							try {
								entries = await readdir(dir);
							} catch {
								continue;
							}
							for (const entry of entries.sort()) {
								const file = path.join(dir, entry, "SKILL.md");
								let parsed;
								try {
									parsed = parseSkill(await readFile(file, "utf8"));
								} catch {
									continue;
								}
								if (parsed === void 0) continue;
								found.push({
									name: parsed.name,
									description: parsed.description,
									...parsed.whenToUse === void 0 ? {} : { whenToUse: parsed.whenToUse },
									invocation: {
										modelInvocable: parsed.modelInvocable,
										userInvocable: parsed.userInvocable
									},
									source: "custom",
									provider: "orch-skills",
									path: file,
									resourceBase: {
										kind: "directory",
										path: path.dirname(file)
									},
									rank: CUSTOM_RANK,
									locator: file
								});
							}
						}
						return found;
					},
					async get(candidate) {
						const parsed = parseSkill(await readFile(candidate.locator, "utf8"));
						if (parsed === void 0 || parsed.name !== candidate.name) return void 0;
						return {
							...candidate,
							content: parsed.body
						};
					}
				};
			});
			const learn = async (sessionId) => {
				const dir = host.book.metaOf(sessionId)?.skills_dir;
				if (typeof dir !== "string" || dir === "") return;
				const own = host.service("dshHomePath");
				const resolved = await realOrSame(dir);
				if (typeof own === "function" && resolved === await realOrSame(path.join(own(), "skills"))) return;
				if (directories.has(resolved)) return;
				directories.add(resolved);
				invalidate?.();
			};
			const note = (sessionId) => void learn(sessionId).catch((error) => host.failRuntime("skills", error));
			host.book.onOpened(({ sessionId }) => note(sessionId));
			host.book.onPrompt((sessionId) => note(sessionId));
		}
		host.channel.handle(Method.skillsRefresh, async (params) => {
			requireOn(host, "skills");
			const input = paramsOf(params);
			const agents = host.service("agents");
			const agent = typeof input["sessionId"] === "string" ? agents?.get(input["sessionId"]) : void 0;
			const options = {};
			if (agent !== void 0) {
				options.scope = agent;
				const cwd = agent.session.header.cwd;
				if (cwd !== void 0) options.cwd = cwd;
			}
			return { skills: (await registry.list(options)).map((s) => s.description === void 0 ? { name: s.name } : {
				name: s.name,
				description: s.description
			}) };
		});
	}
};

//#endregion
//#region src/extras/steering.ts
/**
* Steering and follow-up.
*
* ACP allows one prompt at a time and has no way to speak to a turn that is running. The agent handle of dsh does:
* `steer` gives the model a message at its next step, `followup` queues a message for after the turn. Orch sends both
* as requests, and learns what became of each message from `_orch/input/state`.
*/
/** How many ids of Orch's messages are remembered; older ones were long since claimed. */
const REMEMBERED = 1024;
function textOf(content) {
	if (!Array.isArray(content) || content.length === 0) throw new RpcFault(ErrorCode.invalidParams, "content must be a non-empty list");
	const parts = content;
	const out = [];
	for (const part of parts) if (part?.type === "text" && typeof part.text === "string") out.push({
		type: "text",
		text: part.text
	});
	else if (part?.type === "image") throw new RpcFault(ErrorCode.invalidParams, "an image cannot be sent to a running turn; send it with the next prompt");
	else throw new RpcFault(ErrorCode.invalidParams, "a content part must be text");
	if (out.every((part) => part.text.trim().length === 0)) throw new RpcFault(ErrorCode.invalidParams, "the message is empty");
	return out;
}
const steering = {
	id: "steering",
	start(host) {
		/** dsh's id of a message, by the id Orch gave it; the reverse is looked up when the state is reported. */
		const orchIdOf = /* @__PURE__ */ new Map();
		const send = (kind) => (params) => {
			requireOn(host, "steering");
			const input = paramsOf(params);
			const agent = liveAgent(host, input.sessionId);
			const orchId = stringParam(input, "messageId");
			const message = {
				role: "user",
				id: randomUUID(),
				content: textOf(input.content),
				source: { kind: "user" }
			};
			orchIdOf.set(message.id, orchId);
			if (orchIdOf.size > REMEMBERED) {
				const oldest = orchIdOf.keys().next().value;
				if (oldest !== void 0) orchIdOf.delete(oldest);
			}
			if (kind === "steer") agent.steer(message);
			else agent.followup(message);
			return {
				messageId: message.id,
				accepted: true
			};
		};
		host.channel.handle(Method.inputSteer, send("steer"));
		host.channel.handle(Method.inputFollowup, send("followup"));
		const report = (state) => host.guarded("steering", (payload) => {
			const orchId = orchIdOf.get(payload.message.id);
			const sessionId = sessionIdOf(payload.agent);
			if (orchId === void 0 || sessionId === void 0) return;
			const notice = {
				sessionId,
				messageId: orchId,
				state
			};
			if (state === "claimed" && typeof payload.turn === "number") notice["turn"] = payload.turn;
			host.channel.notify(Method.inputState, notice);
			if (state !== "queued") orchIdOf.delete(payload.message.id);
		});
		host.ctx.on("agent/inbox/inserted", report("queued"), { global: true });
		host.ctx.on("agent/inbox/claimed", report("claimed"), { global: true });
		host.ctx.on("agent/inbox/discarded", report("discarded"), { global: true });
	}
};

//#endregion
//#region src/extras/attempts.ts
/** The book of this process. */
function attemptBook(host) {
	return host.shared("attempts", () => ({ latest: /* @__PURE__ */ new Map() }));
}
/** The attempt id of the call a message of this turn and step came from, when streaming saw it. */
function attemptOf(host, sessionId, turn, step) {
	const call = attemptBook(host).latest.get(sessionId);
	return call !== void 0 && call.turn === turn && call.step === step ? call.attempt : void 0;
}

//#endregion
//#region src/extras/streaming.ts
/** The attempt that tags each committed message, by the message's id. */
const TAG_LIMIT = 256;
const streaming = {
	id: "streaming",
	start(host) {
		const calls = /* @__PURE__ */ new Map();
		/** The latest attempt of each session, which is the one a committed message of that session comes from. */
		const book = attemptBook(host);
		const tags = /* @__PURE__ */ new Map();
		const { flushMs, flushChars } = host.config.streaming;
		const flush = (call) => {
			if (call.timer !== void 0) clearTimeout(call.timer);
			call.timer = void 0;
			if (call.pending.length === 0) return;
			const frames = call.pending;
			call.pending = [];
			call.pendingChars = 0;
			const seq = call.next;
			call.next += frames.length;
			host.channel.notify(Method.stream, {
				sessionId: call.sessionId,
				turn: call.turn,
				step: call.step,
				attempt: call.attempt,
				seq,
				frames
			});
		};
		const push = (call, frame, chars) => {
			const last = call.pending[call.pending.length - 1];
			if (last !== void 0 && frame.kind === last.kind && (frame.kind === "text" || frame.kind === "reasoning")) {
				const previous = last;
				if (previous.index === frame.index) {
					previous.delta += frame.delta;
					call.pendingChars += chars;
					return after(call);
				}
			}
			if (last !== void 0 && frame.kind === "tool_input" && last.kind === "tool_input" && last.callId === frame.callId) {
				last.delta += frame.delta;
				if (last.name === void 0 && frame.name !== void 0) last.name = frame.name;
				call.pendingChars += chars;
				return after(call);
			}
			call.pending.push(frame);
			call.pendingChars += chars;
			after(call);
		};
		const after = (call) => {
			if (call.pendingChars >= flushChars || flushMs === 0) return flush(call);
			call.timer ??= setTimeout(() => flush(call), flushMs);
		};
		host.ctx.on("agent/assistant-stream", host.guarded("streaming", (payload) => {
			const frame = payload.frame;
			const sessionId = sessionIdOf(payload.agent);
			if (sessionId === void 0) return;
			if (frame.type === "start") {
				const call = {
					sessionId,
					turn: frame.turn,
					step: frame.step,
					attempt: String(frame.attemptId),
					next: 0,
					pending: [],
					pendingChars: 0,
					timer: void 0
				};
				calls.set(call.attempt, call);
				book.latest.set(sessionId, {
					attempt: call.attempt,
					turn: call.turn,
					step: call.step
				});
				push(call, { kind: "start" }, 0);
				return;
			}
			const call = calls.get(String(frame.attemptId));
			if (call === void 0) return;
			if (frame.type === "chunk") {
				const chunk = frame.chunk;
				if (chunk.type === "text-delta") push(call, {
					kind: "text",
					index: chunk.index,
					delta: chunk.text
				}, chunk.text.length);
				else if (chunk.type === "reasoning-delta") push(call, {
					kind: "reasoning",
					index: chunk.index,
					delta: chunk.text
				}, chunk.text.length);
				else if (chunk.type === "tool-call-delta") {
					const piece = {
						kind: "tool_input",
						index: chunk.index,
						callId: String(chunk.id),
						delta: chunk.argumentsDelta
					};
					if (chunk.name !== void 0) piece.name = chunk.name;
					push(call, piece, chunk.argumentsDelta.length);
				}
				return;
			}
			if (frame.type === "end") {
				call.pending.push({
					kind: "end",
					outcome: frame.outcome.kind === "committed" ? "committed" : "abandoned"
				});
				flush(call);
				calls.delete(call.attempt);
			}
		}), { global: true });
		host.ctx.on("session/event", host.guarded("streaming", (session, event) => {
			if (event.type !== "assistant/message") return;
			const sessionId = sessionIdOf(session);
			const messageId = event.data?.message?.id;
			if (sessionId === void 0 || typeof messageId !== "string") return;
			const call = book.latest.get(sessionId);
			if (call === void 0 || call.turn !== event.data.turn || call.step !== event.data.step) return;
			tags.set(messageId, {
				attempt: call.attempt,
				turn: call.turn,
				step: call.step
			});
			if (tags.size > TAG_LIMIT) {
				const oldest = tags.keys().next().value;
				if (oldest !== void 0) tags.delete(oldest);
			}
		}), { global: true });
		host.channel.tapOutgoing((message) => {
			if (!isRequest(message) || message.method !== "session/update") return message;
			const update = message.params?.update;
			if (update === void 0 || typeof update.messageId !== "string") return message;
			if (update.sessionUpdate !== "agent_message_chunk" && update.sessionUpdate !== "agent_thought_chunk") return message;
			const tag = tags.get(update.messageId);
			if (tag === void 0 || !host.isOn("streaming")) return message;
			update._meta = {
				...update._meta,
				[COMMIT_META_KEY]: tag
			};
			return message;
		});
	}
};

//#endregion
//#region src/extras/usage.ts
/** Copies the counts a provider reported; a count it did not report stays absent. */
function mapUsage(raw) {
	if (typeof raw !== "object" || raw === null) return void 0;
	const source = raw;
	const count = (key) => {
		const value = source[key];
		return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : void 0;
	};
	const inputTokens = count("inputTokens");
	const outputTokens = count("outputTokens");
	if (inputTokens === void 0 || outputTokens === void 0) return void 0;
	const usage = {
		inputTokens,
		outputTokens
	};
	const cacheRead = count("cacheReadTokens");
	const cacheWrite = count("cacheWriteTokens");
	const reasoning = count("reasoningTokens");
	const total = count("totalTokens");
	if (cacheRead !== void 0) usage.cacheReadTokens = cacheRead;
	if (cacheWrite !== void 0) usage.cacheWriteTokens = cacheWrite;
	if (reasoning !== void 0) usage.reasoningTokens = reasoning;
	if (total !== void 0) usage.totalTokens = total;
	return usage;
}
/** The usage chunk inside the compact stream of an attempt that committed no message. */
function usageOfAttempt(stream) {
	if (!Array.isArray(stream)) return void 0;
	for (const record of stream) if (record?.type === "chunk" && record.chunk?.type === "usage") return mapUsage(record.chunk.usage);
}
const usage = {
	id: "usage",
	start(host) {
		host.ctx.on("session/event", host.guarded("usage", (session, event) => {
			if (event.type !== "assistant/message" && event.type !== "assistant/attempt") return;
			const own = sessionIdOf(session);
			if (own === void 0) return;
			const sessionId = rootSessionId(host, session);
			const data = event.data;
			const source = data.message?.source;
			const notice = {
				sessionId,
				turn: data.turn,
				step: data.step,
				provider: typeof source?.provider === "string" ? source.provider : "",
				model: typeof source?.model === "string" ? source.model : ""
			};
			const attempt = attemptOf(host, own, data.turn, data.step);
			if (attempt !== void 0) notice.attempt = attempt;
			if (sessionId !== own) notice.subagent = true;
			if (event.type === "assistant/message") {
				const counted = mapUsage(data.usage);
				if (counted !== void 0) notice.usage = counted;
				if (data.interrupted === true) notice.interrupted = true;
			} else {
				const counted = usageOfAttempt(data.stream);
				if (counted === void 0) return;
				notice.usage = counted;
				notice.failed = true;
			}
			host.channel.notify(Method.usage, notice);
		}), { global: true });
	}
};

//#endregion
//#region src/extras/index.ts
const EXTRAS = [
	streaming,
	steering,
	questions,
	plan,
	approvals,
	usage,
	compaction,
	skills,
	fork,
	history,
	backends
];

//#endregion
//#region src/takeover.ts
/**
* Takes over the stdio of the process, so that the plugin sits between dsh's ACP server and the real streams.
*
* The server asks for `process.stdin` and `process.stdout` when it starts, and the plugin's module is evaluated before
* any row of the profile starts, so replacing both properties here is enough: the server reads and writes what the
* channel gives it. Nothing is changed for a process that is not serving the protocol (a help, a version or a
* configuration dump, which print to stdout and must keep doing so).
*/
const TAKEN = Symbol.for("orch-dsh-bridge.stdio");
/** Flags of dsh's launcher that print to stdout and end; the plugin leaves those runs alone. */
const PRINTING_FLAGS = [
	"--help",
	"-h",
	"--version",
	"-V",
	"--dump-config",
	"--dump-config-schema",
	"--dump-default-config"
];
/** Whether this process is one that serves the protocol on its stdio. */
function servesProtocol(argv, stdinIsTerminal) {
	if (stdinIsTerminal) return false;
	return !argv.some((word) => PRINTING_FLAGS.includes(word));
}
/**
* Makes the channel for this process and installs it, once. Returns undefined when the process does not serve the
* protocol, or when another copy of the plugin has already taken the stdio.
*/
function takeOverStdio(log) {
	const holder = globalThis;
	if (holder[TAKEN] !== void 0) return void 0;
	if (!servesProtocol(process.argv, process.stdin.isTTY === true)) return void 0;
	const realStdin = process.stdin;
	const realStdout = process.stdout;
	const channel = new Channel({
		input: realStdin,
		output: {
			write: (line) => realStdout.write(line),
			once: (event, listener) => realStdout.once(event, listener)
		},
		log
	});
	Object.assign(channel.serverOutput, {
		isTTY: false,
		fd: 1
	});
	Object.defineProperty(process, "stdin", {
		configurable: true,
		get: () => channel.serverInput
	});
	Object.defineProperty(process, "stdout", {
		configurable: true,
		get: () => channel.serverOutput
	});
	holder[TAKEN] = channel;
	return channel;
}

//#endregion
//#region src/index.ts
/**
* Orch's plugin for DeepSeek Harness.
*
* dsh's Agent Client Protocol server carries sessions, prompts, tool calls, permissions and cancellation, and leaves
* out what Orch needs to treat dsh like its other agent programs: live token streaming, steering, questions, an
* enforced plan mode, the token counts of each model call, compaction phases, per-session skills, fork, history
* carry, and commands and files that run in an environment of Orch's. This plugin adds each of them as an extra,
* in the protocol's own messages where it has them and under `_orch/` where it does not, and tells Orch which extras
* work in `_orch/hello`. An extra that cannot work in a dsh build is off with its reason; the others keep working, and
* the plain protocol is never touched.
*
* It is loaded by a patch file, as a row with an absolute module path:
*
*   - insert:
*       - id: orch-dsh-bridge
*         name: /absolute/path/orch-dsh-bridge.mjs
*         config: {}
*
* Its module takes over the stdio of the process when it is evaluated (see `takeover.ts`), which happens before any
* row is applied; `apply` then starts the extras.
*/
/** Cordis plugin name. */
const name = PLUGIN_NAME;
const log = (line) => {
	process.stderr.write(`${line}\n`);
};
const channel = takeOverStdio(log);
/**
* The plugin entry. Dsh calls it once with its Cordis context and the row's config.
* @param ctx - the Cordis context
* @param rawConfig - the `config` of the row, or undefined
*/
function apply(ctx, rawConfig) {
	const config = resolveConfig(rawConfig);
	if (channel === void 0) {
		log("orch-dsh-bridge: this process does not serve the protocol on its stdio; the plugin does nothing");
		return;
	}
	ctx.provide("orchBridge", { stream: channel.asMessageStream() });
	const bridge = new Bridge(ctx, config, EXTRAS, channel, log);
	queueMicrotask(() => {
		bridge.start().catch((error) => log(`orch-dsh-bridge: start failed: ${errorText(error)}`));
	});
}

//#endregion
export { apply, name };