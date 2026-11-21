/* The HTML starts these reads before its module graph occupies the connection pool. */
// eslint-disable-next-line @typescript-eslint/no-unused-expressions -- The HTML transform appends this function's invocation arguments.
(function (scope, plan) {
  if (scope.__orchStartupReads) return;
  scope.__orchStartupPlan = plan;
  var controller = new scope.AbortController();
  var stopped = false, sessionTaken = false;
  var session;
  var address = !scope.location.search && /^\/thread\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/?$/i.exec(scope.location.pathname);
  var openingPath = address ? '/api/v1/threads/' + address[1] + '/opening' : null;
  var isLogin = /^\/login\/?$/;
  // Every other page asks for its shell, and for the reads this device saw the same address ask for before its first content
  // last time (the application keeps that list: `api/openingPlan.ts`), in the one request that restores its session.
  var opened = scope.location.pathname + scope.location.search;
  // The code and styles that the same page needed before its content, in this release: fetched with the document.
  var keptFiles = [];
  var pageReads = !address && !isLogin.test(scope.location.pathname) ? openingReads(opened) : null;
  // Signing in lands in the frame, on the page this device last saw there: that code follows the form's first paint.
  var landingFiles = [], afterLogin = null;
  if (isLogin.test(scope.location.pathname)) {
    var onward = new URL(scope.location.href).searchParams.get('redirect');
    openingReads(onward && /^\/(?!\/)/.test(onward) ? onward : '/');
    landingFiles = keptFiles; keptFiles = [];
    afterLogin = function () { preload(plan.common || []); preloadKept(landingFiles); };
  }
  function openingReads(at) {
    var reads = ['/me/bootstrap'];
    try {
      var kept = JSON.parse(scope.localStorage.getItem('orch.openings') || 'null');
      var seen = kept && kept.plans && Object.prototype.hasOwnProperty.call(kept.plans, at) ? kept.plans[at] : null;
      if (seen && Array.isArray(seen.reads)) {
        for (var each of seen.reads) if (typeof each === 'string' && /^\/[^/]/.test(each) && reads.indexOf(each) < 0) reads.push(each);
      }
      if (seen && Array.isArray(seen.files) && plan.application && seen.release === plan.application.file) {
        for (var file of seen.files) if (typeof file === 'string' && /^assets\/(?:[\w./-]+\.(?:js|css)|words\/[\w.-]+\.json)$/.test(file)) keptFiles.push(file);
      }
    } catch { /* A device without its storage opens with the shell alone. */ }
    return reads;
  }
  var loginMeta;
  if (isLogin.test(scope.location.pathname)) {
    var policy = scope.document.querySelector('template[data-orch-login-meta]');
    if (policy) {
      try {
        var data = JSON.parse(policy.content.textContent);
        if (data && data.may_create_org === false && typeof data.needs_setup === 'boolean' && typeof data.local_login === 'boolean') loginMeta = data;
      } catch { /* An unavailable edge projection leaves the native metadata request in charge. */ }
      policy.remove();
    }
  }
  var pendingReads = new Map();
  var storagePath = '/api/v1/admin/storage';
  var isStoragePage = /^\/admin\/storage\/?$/;
  var storageController = isStoragePage.test(scope.location.pathname) ? new scope.AbortController() : null;
  var allowShell, settleFirst;
  var content = new Promise(function (resolve) { allowShell = resolve; });
  if (!address) allowShell();
  // The document's first content, whatever page it is: what is only beside a first screen (a summary) waits for it.
  var firstShown = new Promise(function (resolve) { settleFirst = resolve; });
  var released = false, painted = false, dispatchReady = false, frameNeeded = false, releaseScripts;
  var scriptsReady = new Promise(function (resolve) { releaseScripts = resolve; });
  var files = [];
  for (var page of plan.pages) {
    if (new RegExp(page.pattern).test(scope.location.pathname)) { files.push.apply(files, page.files); break; }
  }
  var held = new Set();
  for (var link of scope.document.querySelectorAll('link[rel="modulepreload"]')) held.add(link.getAttribute('href'));
  function preload(files) {
    for (var file of files) {
      var href = '/' + file;
      if (held.has(href)) continue;
      held.add(href);
      var preload = scope.document.createElement('link');
      preload.rel = 'modulepreload'; preload.href = href; preload.crossOrigin = 'anonymous'; preload.fetchPriority = 'low';
      scope.document.head.appendChild(preload);
    }
  }
  function release(frame) {
    dispatchReady = true; frameNeeded = frameNeeded || frame;
    if (!painted) return;
    if (frameNeeded) preload(plan.common);
    if (released) return;
    released = true; preload(files); releaseScripts();
  }
  function read(path, options) {
    var sentAt = Date.now();
    return scope.fetch(path, Object.assign({ credentials: 'same-origin', signal: controller.signal, priority: 'high' }, options))
      .then(function (response) { return { response: response, sentAt: sentAt, receivedAt: Date.now() }; });
  }
  function leavePage() {
    if (storageController) storageController.abort();
    pendingReads.delete(storagePath);
  }
  function cancel() {
    stopped = true; controller.abort(); session = null;
    pendingReads.clear(); provider.contentShown(); provider.firstContent();
    release(false);
    if (scope.__orchStartupReads === provider) delete scope.__orchStartupReads;
  }
  function bind(request) {
    if (request.signal.aborted) cancel();
    else request.signal.addEventListener('abort', cancel, { once: true });
  }
  var provider = {
    visit: function (path) {
      if (!isLogin.test(path)) loginMeta = undefined;
      if (!isStoragePage.test(path)) leavePage();
    },
    get hasLoginMeta() { return !!loginMeta && !stopped; },
    scriptsReady: scriptsReady,
    application: plan.application,
    painted: function () {
      painted = true; if (dispatchReady) release(frameNeeded);
      if (afterLogin) { var later = afterLogin; afterLogin = null; later(); }
    },
    cancel: cancel,
    afterThreadContent: address ? content : undefined,
    contentShown: function () { provider.afterThreadContent = undefined; allowShell(); },
    openingAddress: opened,
    afterFirstContent: firstShown,
    firstContent: function () { provider.afterFirstContent = undefined; settleFirst(); },
    openingDone: function () {
      // What the first screen asked for has been asked; an answer nobody took is not kept for a later read.
      for (var [key, pending] of pendingReads) {
        if (pending.page && !pending.taken) { pending.controller.abort(); pendingReads.delete(key); }
      }
    },
    take: function (request) {
      if (stopped) return null;
      var url = new URL(request.url, scope.location.href);
      if (url.origin !== scope.location.origin) return null;
      if (!url.search && request.method === 'POST' && url.pathname === '/api/v1/auth/session' && !sessionTaken && request.headers.get('x-orch-csrf') === '1') {
        sessionTaken = true; bind(request);
        var result = session; session = null; return result;
      }
      if (!url.search && loginMeta && request.method === 'GET' && url.pathname === '/api/v1/meta' && !request.headers.has('authorization') && !request.signal.aborted) {
        var metadata = loginMeta; loginMeta = undefined;
        var now = Date.now();
        return Promise.resolve({ response: Response.json(metadata), sentAt: now, receivedAt: now });
      }
      var key = url.pathname + url.search;
      var pending = pendingReads.get(key);
      if (request.method !== 'GET' || !pending || pending.taken) return null;
      pending.taken = true;
      var abort = function () { pending.controller.abort(); };
      if (request.signal.aborted) abort();
      else request.signal.addEventListener('abort', abort, { once: true });
      return pending.answer.then(function (reply) {
        if (stopped || pending.controller.signal.aborted || !reply || request.headers.get('authorization') !== reply.authorization) { abort(); return null; }
        return { response: reply.response, sentAt: reply.sentAt, receivedAt: reply.receivedAt };
      }).finally(function () { pendingReads.delete(key); });
    }
  };
  scope.__orchStartupReads = provider;
  if (scope.performance) scope.performance.mark('orch/start/html-session-asked');
  function project(receipt, body, problem, retry) {
    var headers = new Headers();
    for (var name of ['date', 'x-request-id', 'cache-control']) {
      var value = receipt.response.headers.get(name);
      if (value) headers.set(name, value);
    }
    if (problem) {
      headers.set('content-type', 'application/problem+json');
      if (body.request_id) headers.set('x-request-id', body.request_id);
      if (typeof retry === 'number') headers.set('retry-after', String(retry));
    }
    return { response: Response.json(body, { status: problem ? body.status : 200, headers: headers }), sentAt: receipt.sentAt, receivedAt: receipt.receivedAt };
  }
  // A page's answer is a line of JSON for the session, then one for each read as soon as it is ready, by its position.
  function pageLines(asked, count) {
    var settleSession, failSession, settles = [], reads = [];
    for (var index = 0; index < count; index += 1) reads.push(new Promise(function (resolve) { settles.push(resolve); }));
    var answered = new Promise(function (resolve, reject) { settleSession = resolve; failSession = reject; });
    asked.then(function (receipt) {
      if (!receipt.response.ok || !receipt.response.body) { settleSession(receipt); return undefined; }
      var reader = receipt.response.body.getReader(), decoder = new scope.TextDecoder(), rest = '';
      var line = function (text) {
        var part;
        try { part = JSON.parse(text); } catch { return; }
        if (part && Object.prototype.hasOwnProperty.call(part, 'session')) settleSession(project(receipt, part.session));
        else if (part && typeof part.read === 'number' && settles[part.read]) {
          settles[part.read](part.status === 'ready' ? project(receipt, part.data) : project(receipt, part.problem, true, part.retry_after));
        }
      };
      var next = function () {
        return reader.read().then(function (step) {
          rest += step.done ? decoder.decode() : decoder.decode(step.value, { stream: true });
          for (var at = rest.indexOf('\n'); at >= 0; at = rest.indexOf('\n')) { line(rest.slice(0, at)); rest = rest.slice(at + 1); }
          if (!step.done) return next();
          if (rest) line(rest);
          // An answer that ended before its session line is a failure of the session itself.
          settleSession(project(receipt, { type: 'orch:error:internal', code: 'internal', title: '', detail: '', status: 502, request_id: '' }, true));
          return undefined;
        });
      };
      return next();
    }).catch(failSession).finally(function () { for (var settle of settles) settle(null); });
    return { session: answered, reads: reads };
  }
  var post = { method: 'POST', headers: { 'x-orch-csrf': '1' } };
  var pageHeaders = { 'x-orch-csrf': '1', 'x-orch-view': opened };
  try { pageHeaders['x-orch-timezone'] = scope.Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* The optional timezone hint can be unavailable. */ }
  var initial = pageReads
    ? read('/api/v1/auth/session/open?' + pageReads.map(function (path) { return 'read=' + encodeURIComponent(path); }).join('&'), { method: 'POST', headers: pageHeaders })
    : read(address ? '/api/v1/auth/session/thread/' + address[1] : '/api/v1/auth/session', post);
  var answers = pageReads ? pageLines(initial, pageReads.length) : null;
  var compound = address ? initial.then(function (receipt) {
    if (!receipt.response.ok) { provider.contentShown(); return { receipt: receipt, body: null }; }
    return receipt.response.json().then(function (body) {
      if (!body || body.opening.status !== 'ready') provider.contentShown();
      return { receipt: receipt, body: body };
    });
  }) : null;
  session = compound ? compound.then(function (part) {
    return part.receipt.response.ok ? project(part.receipt, part.body && part.body.session) : part.receipt;
  }) : answers ? answers.session : initial;
  var grant = session.then(function (reply) {
    if (!reply.response.ok || stopped) return null;
    return reply.response.clone().json().then(function (data) {
      return data && data.user && typeof data.user.id === 'string' && typeof data.access_token === 'string' ? data : null;
    });
  });
  var authenticated = grant.then(function (data) {
    if (!data) return null;
    var headers = { Authorization: 'Bearer ' + data.access_token };
    try { headers['x-orch-timezone'] = scope.Intl.DateTimeFormat().resolvedOptions().timeZone; } catch { /* The optional timezone hint can be unavailable. */ }
    return headers;
  });
  function keep(path, own, answer, page) {
    controller.signal.addEventListener('abort', function () { own.abort(); }, { once: true });
    pendingReads.set(path, { controller: own, answer: answer, taken: false, page: page === true });
    answer.catch(function () { return undefined; });
  }
  if (answers) {
    pageReads.forEach(function (path, index) {
      var own = new scope.AbortController();
      keep('/api/v1' + path, own, Promise.all([grant, answers.reads[index]]).then(function (both) {
        if (!both[0] || !both[1] || stopped || own.signal.aborted) return null;
        both[1].authorization = 'Bearer ' + both[0].access_token;
        return both[1];
      }), true);
    });
  }
  if (compound) {
    var openingController = new scope.AbortController();
    keep(openingPath, openingController, compound.then(function (part) {
      if (!part.body || stopped || openingController.signal.aborted) return null;
      var opening = part.body.opening;
      var reply = opening.status === 'ready' ? project(part.receipt, opening.data) : project(part.receipt, opening.problem, true, opening.retry_after);
      reply.authorization = 'Bearer ' + part.body.session.access_token;
      return reply;
    }));
    // The dispatched address already identifies the public feature code while its data is in flight.
    release(true);
  }
  var shellController = new scope.AbortController();
  if (!answers) keep('/api/v1/me/bootstrap', shellController, authenticated.then(function (headers) {
    if (!headers || stopped) return null;
    return content.then(function () {
      if (stopped || shellController.signal.aborted) return null;
      return read('/api/v1/me/bootstrap', { method: 'GET', headers: headers, signal: shellController.signal }).then(function (reply) {
        reply.authorization = headers.Authorization; return reply;
      });
    });
  }));
  if (storageController) {
    keep(storagePath, storageController, grant.then(function (data) {
      if (!data || stopped || storageController.signal.aborted || data.user.is_system_admin !== true) return null;
      // Storage checks this session's current admin mode on the server.
      var authorization = 'Bearer ' + data.access_token;
      return read(storagePath, { method: 'GET', headers: { Authorization: authorization, 'x-orch-view': scope.location.pathname }, signal: storageController.signal }).then(function (reply) {
        reply.authorization = authorization; return reply;
      });
    }));
  }
  authenticated.then(function (headers) { release(!!headers && !stopped); }, function () { provider.contentShown(); release(false); });
  preload(plan.entry || []);
  function preloadKept(list) {
    for (var kept of list) {
      if (/\.js$/.test(kept)) { preload([kept]); continue; }
      if (held.has('/' + kept)) continue;
      held.add('/' + kept);
      // Styles are applied by the code that needs them, and words are read with fetch: both are only fetched here.
      var keptFile = scope.document.createElement('link');
      keptFile.rel = 'preload'; keptFile.href = '/' + kept;
      if (/\.json$/.test(kept)) { keptFile.setAttribute('as', 'fetch'); keptFile.crossOrigin = 'anonymous'; } else keptFile.setAttribute('as', 'style');
      scope.document.head.appendChild(keptFile);
    }
  }
  preloadKept(keptFiles);
  // The sign-in page's words come with its document, in the language this device shows: nobody is known yet.
  if (isLogin.test(scope.location.pathname) && plan.words) {
    var wordParts = [plan.words.initial];
    for (var wordsPattern of Object.keys(plan.words.pages || {})) {
      if (new RegExp(wordsPattern).test(scope.location.pathname)) { wordParts = wordParts.concat(plan.words.pages[wordsPattern]); break; }
    }
    var wordsLocale = deviceLocale();
    preloadKept(wordParts.map(function (part) { return 'assets/words/' + wordsLocale + '-' + part + '.json'; }));
  }
  function deviceLocale() {
    try {
      var stored = JSON.parse(scope.localStorage.getItem('orch.appearance') || 'null');
      if (stored && (stored.locale === 'en' || stored.locale === 'zh-CN')) return stored.locale;
    } catch { /* The browser's own languages decide. */ }
    for (var language of (scope.navigator && scope.navigator.languages) || []) {
      var lower = String(language).toLowerCase();
      if (lower.indexOf('zh') === 0) return 'zh-CN';
      if (lower.indexOf('en') === 0) return 'en';
    }
    return 'en';
  }
  if (isLogin.test(scope.location.pathname)) {
    preload(files);
    if (plan.application) {
      preload([plan.application.file]);
      var styles = new Set();
      for (var style of scope.document.querySelectorAll('link[rel="stylesheet"], link[rel="preload"][as="style"]')) styles.add(style.getAttribute('href'));
      for (var file of plan.application.styles) {
        var href = '/' + file;
        if (styles.has(href)) continue;
        styles.add(href);
        var styleLink = scope.document.createElement('link');
        styleLink.rel = 'preload'; styleLink.setAttribute('as', 'style'); styleLink.href = href;
        scope.document.head.appendChild(styleLink);
      }
    }
    release(false);
  }
  var navigation = scope.performance && scope.performance.getEntriesByType ? scope.performance.getEntriesByType('navigation')[0] : null;
  if (navigation && /^(h2|h3)/.test(navigation.nextHopProtocol)) release(false);
})
