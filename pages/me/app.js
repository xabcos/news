"use strict";

const $ = (selector, root = document) => root.querySelector(selector);

const POLL_MS = 5_000;
const PAYMENT_POLL_MAX_MS = 10 * 60 * 1_000;
const STATUS_REFRESH_MS = 60_000;
const DEFAULT_LIMITS = Object.freeze({
  free: { maxTopics: 1, minInterval: 120, dailyRuns: 12 },
  pro: { maxTopics: 8, minInterval: 60, dailyRuns: 48 },
});
const FALLBACK_PLANS = Object.freeze([
  {
    code: "free",
    name: "Free",
    price_fen: 0,
    validity_days: 0,
    description: "1 个主题 · 间隔 ≥ 120 分钟 · 每日 12 次",
    max_topics: 1,
    min_interval_minutes: 120,
    daily_run_cap: 12,
  },
  {
    code: "pro",
    name: "Pro",
    price_fen: 9900,
    validity_days: 31,
    description: "8 个主题 · 间隔 ≥ 60 分钟 · 每日 48 次",
    max_topics: 8,
    min_interval_minutes: 60,
    daily_run_cap: 48,
  },
]);

const state = {
  user: null,
  limits: null,
  usage: null,
  topics: [],
  running: [],
  selectedTopicId: null,
  items: [],
  itemsLoading: false,
  plans: [],
  paymentEnabled: false,
  orders: [],
  editing: null,
  confirmAction: null,
  loginInFlight: false,
  topicLoading: false,
  topicPoll: null,
  statusTimer: null,
  payment: null,
  paymentTimer: null,
  paymentStartedAt: 0,
  paymentAttempts: 0,
  timezone: "Asia/Shanghai",
  sessionEpoch: 0,
};

const el = Object.fromEntries(
  [
    "boot", "authView", "appView", "loginForm", "email", "mailSent", "sentEmail", "devLoginLink", "sendAgain",
    "planBadge", "accountEmail", "logoutButton", "newTopicButton", "emptyCreateButton", "refreshTopicsButton",
    "topicCount", "topicsLoading", "topicsEmpty", "topicsError", "topicList", "itemsPanel", "itemsTitle", "itemsMeta",
    "refreshItemsButton", "itemsLoading", "itemsEmpty", "itemsError", "itemsList", "currentPlan", "planCards",
    "ordersList", "plansEmpty", "refreshUsageButton", "usageReset", "topicsUsageValue", "topicsUsageProgress",
    "topicsUsageMeta", "runsUsageValue", "runsUsageProgress", "runsUsageMeta", "paymentPanel", "paymentContent",
    "closePaymentButton", "topicDialog", "topicForm", "topicDialogEyebrow", "topicDialogTitle", "topicId",
    "topicName", "topicBrief", "topicInterval", "intervalHint", "topicEnabled", "topicFormMessage",
    "saveTopicButton", "confirmDialog", "confirmTitle", "confirmMessage",
    "confirmActionButton", "toastRegion",
  ].map((id) => [id, $(`#${id}`)]),
);

document.addEventListener("DOMContentLoaded", initialize);

async function initialize() {
  bindEvents();
  const token = takeLoginToken();
  try {
    if (token) await finishLogin(token);
    else await restoreSession();
  } catch (error) {
    showAuth();
    if (error.status !== 401 && error.status !== 403) showToast(error.message || "暂时无法打开个人空间", "error");
  } finally {
    window.requestAnimationFrame(() => el.boot?.classList.add("is-ready"));
  }
}

function bindEvents() {
  el.loginForm?.addEventListener("submit", requestLogin);
  el.sendAgain?.addEventListener("click", () => {
    resetLoginForm();
    el.email?.focus();
  });
  el.logoutButton?.addEventListener("click", logout);
  el.newTopicButton?.addEventListener("click", () => openTopicDialog());
  el.emptyCreateButton?.addEventListener("click", () => openTopicDialog());
  el.refreshTopicsButton?.addEventListener("click", () => loadTopics());
  el.refreshItemsButton?.addEventListener("click", () => {
    if (state.selectedTopicId) loadTopicItems(state.selectedTopicId);
  });
  el.refreshUsageButton?.addEventListener("click", () => refreshAccount());
  el.topicForm?.addEventListener("submit", saveTopic);
  el.topicInterval?.addEventListener("input", validateIntervalHint);
  el.closePaymentButton?.addEventListener("click", hidePayment);
  el.confirmActionButton?.addEventListener("click", runConfirmedAction);
  document.querySelectorAll("[data-close-dialog]").forEach((button) => {
    button.addEventListener("click", () => closeDialog(button.closest("dialog")));
  });
  document.querySelectorAll("dialog").forEach(bindDialogDismissal);
}

// ---------------------------------------------------------------------------
// Session and transport
// ---------------------------------------------------------------------------

async function api(path, options = {}) {
  // This hint controls client-side session handling and is not a Fetch option.
  const { skipAuthRedirect = false, ...requestOptions } = options;
  let response;
  try {
    response = await fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      ...requestOptions,
      headers: {
        ...(typeof requestOptions.body === "string" ? { "Content-Type": "application/json" } : {}),
        ...(requestOptions.headers || {}),
      },
    });
  } catch {
    throw new Error("网络连接失败，请检查网络后重试");
  }
  const type = response.headers.get("content-type") || "";
  const payload = type.includes("application/json") ? await response.json().catch(() => null) : null;
  if (!response.ok) {
    const error = new Error(payload?.error?.message || payload?.message || `请求失败（${response.status}）`);
    error.status = response.status;
    error.data = payload;
    if (state.user && !skipAuthRedirect && [401, 403].includes(response.status)) {
      showAuth();
      showToast("登录状态已失效，请重新登录", "error");
    }
    throw error;
  }
  return payload;
}

async function restoreSession() {
  try {
    await enterApp(await api("/api/me"));
  } catch (error) {
    if (![401, 403].includes(error.status)) throw error;
    showAuth();
  }
}

async function requestLogin(event) {
  event.preventDefault();
  const email = el.email.value.trim();
  if (!email || !el.email.checkValidity()) {
    el.email.focus();
    return showToast("请输入有效的邮箱地址", "error");
  }
  const button = $("button[type='submit']", el.loginForm);
  setBusy(button, true, "正在发送…");
  try {
    const data = await api("/api/auth/email/start", {
      method: "POST",
      body: JSON.stringify({ email }),
      skipAuthRedirect: true,
    });
    el.sentEmail.textContent = data?.email || email;
    el.loginForm.classList.add("hidden");
    el.mailSent.classList.remove("hidden");
    const loginUrl = safeLoginUrl(data?.login_url);
    el.devLoginLink.classList.toggle("hidden", !loginUrl);
    if (loginUrl) el.devLoginLink.href = loginUrl;
    else el.devLoginLink.removeAttribute("href");
    window.requestAnimationFrame(() => (loginUrl || el.sendAgain).focus());
  } catch (error) {
    showToast(error.message, "error");
  } finally {
    setBusy(button, false, "继续");
  }
}

async function finishLogin(token) {
  if (!token || state.loginInFlight) return;
  state.loginInFlight = true;
  try {
    await enterApp(await api("/api/auth/email/consume", {
      method: "POST",
      body: JSON.stringify({ token }),
      skipAuthRedirect: true,
    }));
    showToast("已登录 NEWS");
  } finally {
    state.loginInFlight = false;
  }
}

async function enterApp(session) {
  const source = session || {};
  const user = normalizeUser(source.user || source);
  if (!user.email) throw new Error("登录状态无效，请重新登录");
  if (user.role === "admin") {
    window.location.replace("/admin/");
    return;
  }
  state.sessionEpoch += 1;
  const epoch = state.sessionEpoch;
  state.user = user;
  syncEntitlements(source);
  showApp();
  const tasks = [loadTopics(), loadStatus(), loadPlans(), loadOrders()];
  await Promise.allSettled(tasks);
  if (epoch !== state.sessionEpoch || !state.user) return;
  renderAll();
  if (!state.statusTimer) state.statusTimer = window.setInterval(() => {
    if (state.user && document.visibilityState === "visible") refreshTopicsAndUsage(true);
  }, STATUS_REFRESH_MS);
}

function showApp() {
  el.authView.classList.add("hidden");
  el.appView.classList.remove("hidden");
  renderAccountHeader();
  updateFormLimits();
}

function renderAccountHeader() {
  if (!state.user) return;
  el.accountEmail.textContent = state.user.email;
  const pro = isPro();
  el.planBadge.textContent = pro ? "Pro" : "Free";
  el.planBadge.classList.toggle("is-pro", pro);
}

function showAuth() {
  state.sessionEpoch += 1;
  stopTopicPolling();
  stopPaymentPolling();
  if (state.statusTimer) window.clearInterval(state.statusTimer);
  state.statusTimer = null;
  state.user = null;
  state.limits = null;
  state.usage = null;
  state.topics = [];
  state.running = [];
  state.selectedTopicId = null;
  state.items = [];
  state.itemsLoading = false;
  state.plans = [];
  state.orders = [];
  state.paymentEnabled = false;
  state.timezone = "Asia/Shanghai";
  state.editing = null;
  el.topicList?.replaceChildren();
  el.ordersList?.replaceChildren();
  el.ordersList?.classList.add("hidden");
  el.currentPlan?.replaceChildren();
  el.planCards?.replaceChildren();
  el.topicsError?.classList.add("hidden");
  if (el.itemsPanel) el.itemsPanel.classList.add("hidden");
  if (el.itemsList) el.itemsList.replaceChildren();
  el.appView.classList.add("hidden");
  el.authView.classList.remove("hidden");
  el.accountEmail.textContent = "";
  resetLoginForm();
  closeDialog(el.topicDialog);
  closeDialog(el.confirmDialog);
  hidePayment();
  renderAll();
}

function resetLoginForm() {
  el.mailSent.classList.add("hidden");
  el.loginForm.classList.remove("hidden");
  el.sentEmail.textContent = "";
  el.devLoginLink.classList.add("hidden");
  el.devLoginLink.removeAttribute("href");
  setBusy($("button[type='submit']", el.loginForm), false, "继续");
}

async function logout() {
  setBusy(el.logoutButton, true, "退出中…");
  try {
    await api("/api/auth/logout", { method: "POST", skipAuthRedirect: true });
    showAuth();
  } catch (error) {
    showToast(error.message || "暂时无法退出，请稍后重试", "error");
  } finally {
    setBusy(el.logoutButton, false, "退出");
  }
}

// ---------------------------------------------------------------------------
// Entitlements, topics and usage
// ---------------------------------------------------------------------------

function normalizeUser(value) {
  const user = value && typeof value === "object" ? value : {};
  return {
    ...user,
    email: String(user.email || "").trim(),
    role: String(user.role || "user").toLowerCase(),
    plan: String(user.plan || user.plan_code || "free").toLowerCase(),
    pro_until: user.pro_until || user.plan_expires_at || user.pro_expires_at || null,
    limits: user.limits || {},
  };
}

function syncEntitlements(source = {}) {
  const user = state.user || {};
  const nestedPlan = source.plan && typeof source.plan === "object" ? source.plan : null;
  const planCode = String(
    nestedPlan?.code || source.plan_code || user.plan || "free",
  ).toLowerCase();
  const proUntil = source.pro_until || source.plan_expires_at || user.pro_until || nestedPlan?.expires_at || null;
  const activePro = planCode === "pro" && (!proUntil || Date.parse(proUntil) > Date.now());
  state.user.plan = activePro ? "pro" : "free";
  state.user.pro_until = proUntil;
  state.limits = normalizeLimits(source.limits || user.limits || source.usage || source.daily_usage || {}, state.user.plan);
  state.usage = normalizeUsage(source.usage || source.daily_usage || source, state.limits);
  if (state.usage.timezone && isTimezone(state.usage.timezone)) state.timezone = state.usage.timezone;
}

function isPro() {
  return state.user?.plan === "pro" && (!state.user.pro_until || Date.parse(state.user.pro_until) > Date.now());
}

function defaultLimits(code = "free") {
  return DEFAULT_LIMITS[code] || DEFAULT_LIMITS.free;
}

function normalizeLimits(value = {}, code = state.user?.plan || "free") {
  const defaults = defaultLimits(code);
  const source = value?.limits && typeof value.limits === "object" ? value.limits : value;
  const number = (keys, fallback) => {
    for (const key of keys) {
      const parsed = Number(source?.[key]);
      if (Number.isFinite(parsed) && parsed >= 0) return parsed;
    }
    return fallback;
  };
  return {
    maxTopics: number(["max_topics", "topics_limit", "topic_limit"], defaults.maxTopics),
    minInterval: number(["min_interval_minutes", "min_interval", "interval_min"], defaults.minInterval),
    dailyRuns: number(["daily_run_cap", "runs_limit", "daily_runs_limit"], defaults.dailyRuns),
  };
}

function normalizeUsage(value = {}, limits = state.limits || defaultLimits()) {
  const source = value?.usage && typeof value.usage === "object" ? value.usage : value;
  const today = source?.today && typeof source.today === "object" ? source.today : {};
  const number = (keys, fallback = null) => {
    for (const key of keys) {
      const parsed = Number(source?.[key] ?? today?.[key]);
      if (Number.isFinite(parsed) && parsed >= 0) return parsed;
    }
    return fallback;
  };
  const topicsUsed = number(["topics_used", "topic_count", "topics_count"], null);
  const runsUsed = number(["runs_used", "runs_today", "daily_runs", "run_count", "runs"], null);
  return {
    topicsUsed,
    topicsLimit: number(["topics_limit", "max_topics", "topic_limit"], limits.maxTopics),
    runsUsed,
    runsLimit: number(["runs_limit", "daily_run_cap", "daily_runs_limit"], limits.dailyRuns),
    resetAt: source?.reset_at || source?.period_end || source?.next_reset_at || null,
    timezone: source?.timezone || "Asia/Shanghai",
  };
}

async function refreshAccount({ quiet = false } = {}) {
  if (!state.user) return;
  const epoch = state.sessionEpoch;
  const userId = state.user.id;
  try {
    const data = await api("/api/me");
    if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
    state.user = normalizeUser(data?.user || data);
    syncEntitlements(data);
    renderAccountHeader();
    renderAll();
  } catch (error) {
    if (epoch === state.sessionEpoch && state.user && state.user.id === userId && !quiet) {
      showToast(error.message, "error");
    }
  }
}

async function refreshTopicsAndUsage(quiet = false) {
  try {
    await Promise.all([loadTopics(state.editing?.id), loadStatus(), refreshAccount({ quiet: true })]);
  } catch (error) {
    if (!quiet) showToast(error.message, "error");
  }
}

async function loadTopics(selectId) {
  if (!state.user) return [];
  const epoch = state.sessionEpoch;
  state.topicLoading = true;
  renderTopicList();
  try {
    const data = await api("/api/me/topics");
    if (epoch !== state.sessionEpoch || !state.user) return state.topics;
    const list = extractList(data, "topics");
    state.topics = list;
    el.topicsError?.classList.add("hidden");
    if (el.topicsError) el.topicsError.textContent = "";
    if (state.selectedTopicId && !state.topics.some((topic) => String(topic.id) === state.selectedTopicId)) {
      state.selectedTopicId = null;
      state.items = [];
      el.itemsPanel?.classList.add("hidden");
    }
    const usage = normalizeUsage(data?.usage || data?.limits || {}, state.limits || defaultLimits());
    if (usage.topicsUsed !== null || usage.runsUsed !== null) state.usage = { ...state.usage, ...usage };
    renderAll();
    if (selectId && state.topics.some((topic) => String(topic.id) === String(selectId))) state.editing = state.topics.find((topic) => String(topic.id) === String(selectId));
    return state.topics;
  } catch (error) {
    if (epoch !== state.sessionEpoch || !state.user) return state.topics;
    showTopicError(error.message);
    throw error;
  } finally {
    if (epoch === state.sessionEpoch) {
      state.topicLoading = false;
      renderTopicList();
    }
  }
}

async function loadStatus() {
  if (!state.user) return;
  const epoch = state.sessionEpoch;
  try {
    const data = await api("/api/me/status");
    if (epoch !== state.sessionEpoch || !state.user) return;
    if (data?.timezone && isTimezone(data.timezone)) state.timezone = data.timezone;
    state.running = Array.isArray(data?.running) ? data.running.map(String) : [];
    renderTopicList();
  } catch (error) {
    if (![404, 405].includes(error.status)) throw error;
  }
}

function topicApi(suffix = "", options = {}) {
  return api(`/api/me/topics${suffix}`, options);
}

function renderTopicList() {
  const count = state.topics.length;
  const limit = state.usage?.topicsLimit ?? state.limits?.maxTopics ?? defaultLimits().maxTopics;
  el.topicCount.textContent = `${count} / ${limit} 个主题`;
  el.newTopicButton.disabled = count >= limit;
  el.newTopicButton.title = count >= limit ? `当前套餐最多 ${limit} 个主题` : "新建主题";
  el.topicsLoading.classList.toggle("hidden", !state.topicLoading);
  el.topicsEmpty.classList.toggle("hidden", state.topicLoading || count > 0);
  el.topicList.classList.toggle("hidden", state.topicLoading || count === 0);
  if (!state.topicLoading) el.topicList.replaceChildren(...state.topics.map(renderTopicRow));
}

function renderTopicRow(topic) {
  const row = document.createElement("li");
  const locked = topic.plan_locked === true;
  row.className = `me-topic-row${locked ? " is-locked" : ""}`;
  const main = document.createElement("div");
  main.className = "me-topic-main";
  const title = document.createElement("div");
  title.className = "me-topic-title";
  const dot = document.createElement("span");
  dot.className = `me-status-dot ${locked ? "is-locked" : topicRunning(topic) ? "is-running" : topic.enabled ? "is-on" : "is-error"}`;
  dot.setAttribute("aria-hidden", "true");
  const name = document.createElement("strong");
  name.textContent = topic.name || "未命名主题";
  const interval = document.createElement("span");
  interval.className = "me-topic-pill";
  interval.textContent = `${Math.max(Number(topic.interval_minutes || 0), Number(state.limits?.minInterval || 120))} 分钟`;
  title.append(dot, name, interval);
  const meta = document.createElement("div");
  meta.className = "me-topic-meta";
  meta.textContent = topicMeta(topic);
  main.append(title, meta);

  const actions = document.createElement("div");
  actions.className = "me-topic-actions";
  const run = topicActionButton(locked ? "需 Pro" : topicRunning(topic) ? "抓取中" : "抓取", "button-secondary", () => runTopic(topic, run));
  run.disabled = locked || topicRunning(topic);
  const view = topicActionButton("新闻", "button-quiet", () => loadTopicItems(topic.id));
  const edit = topicActionButton("编辑", "button-quiet", () => openTopicDialog(topic));
  const remove = topicActionButton("删除", "button-danger", () => confirmDeleteTopic(topic));
  actions.append(run, view, edit, remove);
  row.append(main, actions);
  return row;
}

function topicActionButton(label, style, handler) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `button button-small ${style}`;
  button.textContent = label;
  button.addEventListener("click", handler);
  return button;
}

function topicRunning(topic) {
  return topic?.plan_locked === true ? false : state.running.includes(String(topic?.id))
    || ["running", "queued", "processing"].includes(String(topic?.status || topic?.last_status || "").toLowerCase())
    || topic?.running === true;
}

function topicMeta(topic) {
  if (topic.plan_locked === true) return "当前套餐不包含 · 升级 Pro 后恢复";
  if (!topic.enabled) return "已暂停";
  if (topicRunning(topic)) return "正在抓取…";
  const next = topic.next_run_at || topic.next_run;
  const parts = [];
  if (next) parts.push(`下次 ${shortDateTime(next)}`);
  if (topic.last_status && topic.last_status !== "ok" && topic.last_status !== "success") parts.push(topic.last_status === "failed" ? "上次失败" : String(topic.last_status));
  return parts.join(" · ") || "等待排程";
}

function showTopicError(message) {
  if (!el.topicsError) return;
  el.topicsError.textContent = String(message || "主题暂时无法载入");
  el.topicsError.classList.remove("hidden");
}

async function loadTopicItems(topicId) {
  const epoch = state.sessionEpoch;
  const topic = state.topics.find((item) => String(item.id) === String(topicId));
  if (!topic) return;
  state.selectedTopicId = String(topic.id);
  state.itemsLoading = true;
  el.itemsPanel.classList.remove("hidden");
  el.itemsTitle.textContent = `${topic.name || "主题"} · 最近内容`;
  el.itemsError.classList.add("hidden");
  renderItemsPanel();
  try {
    const data = await topicApi(`/${encodeURIComponent(topic.id)}/items`);
    if (epoch !== state.sessionEpoch || state.selectedTopicId !== String(topic.id)) return;
    state.items = extractList(data, "items");
  } catch (error) {
    if (epoch === state.sessionEpoch && state.selectedTopicId === String(topic.id)) {
      state.items = [];
      el.itemsError.textContent = error.message || "内容暂时无法载入";
      el.itemsError.classList.remove("hidden");
    }
  } finally {
    if (epoch === state.sessionEpoch && state.selectedTopicId === String(topic.id)) {
      state.itemsLoading = false;
      renderItemsPanel();
    }
  }
}

function renderItemsPanel() {
  if (!el.itemsPanel) return;
  const items = state.items || [];
  el.itemsLoading.classList.toggle("hidden", !state.itemsLoading);
  el.itemsEmpty.classList.toggle("hidden", state.itemsLoading || items.length > 0);
  el.itemsList.classList.toggle("hidden", state.itemsLoading || items.length === 0);
  el.itemsMeta.textContent = state.itemsLoading ? "正在载入…" : `${items.length} 条`;
  if (!state.itemsLoading) el.itemsList.replaceChildren(...items.map(renderItemRow));
}

function renderItemRow(item) {
  const row = document.createElement("li");
  row.className = "me-item-row";
  const articleUrl = safeArticleUrl(item.url);
  const link = articleUrl ? document.createElement("a") : document.createElement("strong");
  if (articleUrl) {
    link.href = articleUrl;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
  }
  link.textContent = item.title || "未命名内容";
  const summary = document.createElement("p");
  summary.textContent = item.summary || "";
  const meta = document.createElement("div");
  meta.className = "me-item-meta";
  const source = item.source ? document.createElement("span") : null;
  if (source) source.textContent = item.source;
  // `published_at` may only be a calendar date; show when NEWS actually collected it.
  const date = item.first_seen_at;
  const when = date ? document.createElement("span") : null;
  if (when) when.textContent = shortDateTime(date);
  meta.append(...[source, when].filter(Boolean));
  for (const tag of Array.isArray(item.tags) ? item.tags.slice(0, 4) : []) {
    const node = document.createElement("span");
    node.className = "me-item-tag";
    node.textContent = tag;
    meta.append(node);
  }
  row.append(link, summary, meta);
  return row;
}

// ---------------------------------------------------------------------------
// Topic editor
// ---------------------------------------------------------------------------

function openTopicDialog(topic = null) {
  if (!topic && state.topics.length >= (state.limits?.maxTopics ?? defaultLimits().maxTopics)) {
    return showToast(`当前套餐最多 ${state.limits?.maxTopics ?? 1} 个主题`, "error");
  }
  state.editing = topic;
  const minimum = Math.max(1, Math.round(state.limits?.minInterval ?? defaultLimits().minInterval));
  el.topicDialogEyebrow.textContent = topic ? "编辑主题" : "新主题";
  el.topicDialogTitle.textContent = topic ? "编辑主题" : "创建主题";
  el.topicId.value = topic?.id || "";
  el.topicName.value = topic?.name || "";
  el.topicBrief.value = topic?.brief || topic?.description || "";
  el.topicInterval.min = String(minimum);
  el.topicInterval.value = String(Math.max(minimum, Number(topic?.interval_minutes || minimum)));
  el.topicEnabled.checked = topic?.enabled !== false;
  el.topicFormMessage.classList.add("hidden");
  el.topicFormMessage.textContent = "";
  updateFormLimits();
  if (!el.topicDialog.open) el.topicDialog.showModal();
  document.body.classList.add("dialog-open");
  window.requestAnimationFrame(() => el.topicName.focus());
}

function updateFormLimits() {
  const minimum = Math.max(1, Math.round(state.limits?.minInterval ?? defaultLimits(isPro() ? "pro" : "free").minInterval));
  if (!el.topicInterval) return;
  el.topicInterval.min = String(minimum);
  if (Number(el.topicInterval.value) < minimum) el.topicInterval.value = String(minimum);
  el.intervalHint.textContent = `${isPro() ? "Pro" : "Free"} 最短 ${minimum} 分钟`;
}

function validateIntervalHint() {
  const minimum = Number(el.topicInterval.min || 1);
  const value = Number(el.topicInterval.value);
  el.intervalHint.classList.toggle("me-hint-error", Number.isFinite(value) && value < minimum);
}

async function saveTopic(event) {
  event.preventDefault();
  const name = el.topicName.value.trim();
  const brief = el.topicBrief.value.trim();
  const interval = Number.parseInt(el.topicInterval.value, 10);
  const minimum = Number(el.topicInterval.min || 1);
  if (!name) return showTopicFormMessage("请填写主题名称");
  if (!brief) return showTopicFormMessage("请填写关注方向");
  if (!Number.isInteger(interval) || interval < minimum || interval > 1440) return showTopicFormMessage(`抓取间隔不能小于 ${minimum} 分钟`);

  const topic = state.editing;
  const payload = {
    name,
    brief,
    enabled: el.topicEnabled.checked,
    interval_minutes: interval,
  };
  setBusy(el.saveTopicButton, true, "保存中…");
  try {
    const data = topic
      ? await topicApi(`/${encodeURIComponent(topic.id)}`, { method: "PATCH", body: JSON.stringify(payload) })
      : await topicApi("", { method: "POST", body: JSON.stringify(payload) });
    const saved = data?.topic || data;
    closeDialog(el.topicDialog);
    showToast(topic ? "主题已保存" : "主题已创建");
    await loadTopics(saved?.id || topic?.id);
    await refreshAccount({ quiet: true });
  } catch (error) {
    showTopicFormMessage(topicErrorMessage(error));
  } finally {
    setBusy(el.saveTopicButton, false, "保存主题");
  }
}

function showTopicFormMessage(message) {
  el.topicFormMessage.textContent = String(message);
  el.topicFormMessage.classList.remove("hidden");
}

function confirmDeleteTopic(topic) {
  openConfirm("删除这个主题？", `“${topic.name || "未命名主题"}”及其新闻内容会被删除。`, async () => {
    await topicApi(`/${encodeURIComponent(topic.id)}`, { method: "DELETE" });
    state.editing = null;
    await loadTopics();
    await refreshAccount({ quiet: true });
    showToast("主题已删除");
  });
}

async function runTopic(topic, button) {
  if (!topic?.id || topicRunning(topic)) return;
  state.running = [...new Set([...state.running, String(topic.id)])];
  renderTopicList();
  setBusy(button, true, "提交中…");
  try {
    await topicApi(`/${encodeURIComponent(topic.id)}/run`, { method: "POST" });
    showToast("抓取已开始，稍后回来查看结果");
    await refreshTopicsAndUsage(true);
    pollTopic(topic.id);
  } catch (error) {
    state.running = state.running.filter((id) => id !== String(topic.id));
    renderTopicList();
    showToast(topicErrorMessage(error), "error");
  } finally {
    setBusy(button, false, "抓取");
  }
}

function pollTopic(topicId) {
  stopTopicPolling();
  let remaining = 90;
  state.topicPoll = window.setInterval(async () => {
    if (!state.user || remaining-- <= 0) return stopTopicPolling();
    try {
      await Promise.all([loadTopics(topicId), loadStatus()]);
      const topic = state.topics.find((item) => String(item.id) === String(topicId));
      if (topic && !topicRunning(topic)) {
        stopTopicPolling();
        if (state.selectedTopicId === String(topicId)) await loadTopicItems(topicId);
      }
    } catch {
      // A transient refresh failure should not turn a successful run into an error toast.
    }
  }, POLL_MS);
}

function stopTopicPolling() {
  if (state.topicPoll) window.clearInterval(state.topicPoll);
  state.topicPoll = null;
}

function topicErrorMessage(error) {
  const data = error?.data?.error || error?.data || {};
  const code = String(data.code || "").toLowerCase();
  if (code.includes("topic") && (code.includes("limit") || code.includes("quota"))) return `主题数量已达上限（${state.limits?.maxTopics ?? 1} 个）`;
  if (code.includes("interval") || code.includes("cooldown")) return `抓取间隔不能小于 ${state.limits?.minInterval ?? 120} 分钟`;
  if (code.includes("daily") || code.includes("run_limit")) return `今日运行次数已达上限，请在${resetText()}后重试`;
  if (error?.status === 429) return data.reset_at ? `请求受限，请在${formatReset(data.reset_at)}后重试` : (error.message || "请求过于频繁，请稍后重试");
  return error?.message || "操作未完成，请稍后重试";
}

// ---------------------------------------------------------------------------
// Usage and plans
// ---------------------------------------------------------------------------

async function loadPlans() {
  const epoch = state.sessionEpoch;
  try {
    const data = await api("/api/plans");
    if (epoch !== state.sessionEpoch || !state.user) return;
    state.paymentEnabled = data?.checkout_enabled === true;
    const plans = extractList(data, "plans").filter((plan) => ["free", "pro"].includes(String(plan.code || "").toLowerCase()));
    state.plans = plans.length ? mergePlans(plans) : FALLBACK_PLANS.map((plan) => ({ ...plan }));
  } catch (error) {
    if (epoch !== state.sessionEpoch || !state.user) return;
    if (![404, 405].includes(error.status)) showToast(error.message, "error");
    state.paymentEnabled = false;
    state.plans = FALLBACK_PLANS.map((plan) => ({ ...plan }));
  }
  renderPlans();
}

function mergePlans(plans) {
  const byCode = new Map(plans.map((plan) => [String(plan.code).toLowerCase(), plan]));
  return FALLBACK_PLANS.map((fallback) => ({ ...fallback, ...(byCode.get(fallback.code) || {}) }));
}

function renderPlans() {
  const current = isPro() ? "pro" : "free";
  const currentPlan = state.plans.find((plan) => String(plan.code).toLowerCase() === current) || findFallbackPlan(current);
  const limits = state.limits || normalizeLimits({}, current);
  const expires = isPro() && state.user?.pro_until ? `有效期至 ${formatDate(state.user.pro_until)}` : "当前免费套餐";
  el.currentPlan.innerHTML = "";
  const card = document.createElement("div");
  card.className = "me-current-plan-card";
  const top = document.createElement("div");
  top.className = "me-current-plan-top";
  const name = document.createElement("strong");
  name.textContent = currentPlan?.name || (current === "pro" ? "Pro" : "Free");
  const label = document.createElement("span");
  label.textContent = current === "pro" ? expires : `主题 ${state.topics.length}/${limits.maxTopics}`;
  top.append(name, label);
  const desc = document.createElement("p");
  desc.textContent = current === "pro" ? `主题 ${limits.maxTopics} 个 · 间隔 ≥ ${limits.minInterval} 分钟` : "适合轻量、稳定的每日阅读";
  card.append(top, desc);
  el.currentPlan.append(card);

  const available = state.plans.length ? state.plans : FALLBACK_PLANS;
  el.planCards.replaceChildren(...available.map((plan) => renderPlanCard(plan, current)));
  el.plansEmpty.classList.toggle("hidden", available.length > 0);
}

function renderPlanCard(plan, current) {
  const code = String(plan.code || "free").toLowerCase();
  const limits = normalizeLimits(plan, code);
  const article = document.createElement("article");
  article.className = `me-plan-card${code === current ? " is-current" : ""}`;
  const top = document.createElement("div");
  top.className = "me-plan-card-top";
  const name = document.createElement("strong");
  name.textContent = code === "pro" ? "Pro" : "Free";
  const price = document.createElement("span");
  price.className = "me-plan-price";
  const durationDays = Number(plan.validity_days ?? plan.duration_days ?? 31);
  price.textContent = Number(plan.price_fen || 0) > 0 ? `¥${(Number(plan.price_fen) / 100).toFixed(0)} / ${durationDays} 天` : "免费";
  top.append(name, price);
  const description = document.createElement("p");
  description.className = "me-plan-description";
  description.textContent = plan.description || `${limits.maxTopics} 个主题 · 间隔 ≥ ${limits.minInterval} 分钟 · 每日 ${limits.dailyRuns} 次`;
  article.append(top, description);
  const button = document.createElement("button");
  button.type = "button";
  button.className = `button ${code === "pro" ? "button-primary" : "button-secondary"}`;
  const checkoutAvailable = state.paymentEnabled;
  const isCurrent = code === current;
  const canCheckout = code === "pro" && checkoutAvailable;
  button.textContent = code === current
    ? code === "pro" && checkoutAvailable ? "续费 Pro" : "当前套餐"
    : code === "pro"
      ? checkoutAvailable ? "升级 Pro" : "暂未开放"
      : "Free";
  button.disabled = !canCheckout;
  if (canCheckout) {
    button.addEventListener("click", () => createProOrder(button));
  } else if (code === "pro" && !isCurrent) {
    button.title = "管理员尚未配置 YPay 支付";
  }
  article.append(button);
  return article;
}

async function loadOrders() {
  if (!state.user) return;
  const epoch = state.sessionEpoch;
  try {
    const data = await api("/api/orders");
    if (epoch !== state.sessionEpoch || !state.user) return;
    state.orders = extractList(data, "orders");
    renderOrders();
    const pending = state.orders.find((order) => order.status === "pending" && paymentDetails(order));
    if (pending && !state.payment) showPaymentForOrder(pending, paymentDetails(pending));
  } catch (error) {
    if (epoch !== state.sessionEpoch || !state.user) return;
    // Checkout may be intentionally disabled in local development; keep the
    // rest of the personal space quiet in that case.
    if (![404, 405, 503].includes(error.status)) showToast(error.message, "error");
    state.orders = [];
    renderOrders();
  }
}

function renderOrders() {
  if (!state.orders.length) {
    el.ordersList.classList.add("hidden");
    el.ordersList.replaceChildren();
    return;
  }
  el.ordersList.classList.remove("hidden");
  const title = document.createElement("p");
  title.className = "me-orders-title";
  title.textContent = "最近订单";
  el.ordersList.replaceChildren(title, ...state.orders.slice(0, 3).map((order) => {
    const row = document.createElement("div");
    row.className = "me-order";
    const name = document.createElement("span");
    name.className = "me-order-name";
    name.textContent = `${String(order.plan_code || "Pro").toUpperCase()} · ${formatDate(order.created_at)}`;
    const status = document.createElement("span");
    status.className = `me-order-status is-${String(order.status || "pending").toLowerCase()}`;
    status.textContent = orderStatus(order);
    row.append(name, status);
    if (order.status === "pending" && paymentDetails(order)) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "button button-secondary";
      button.textContent = "继续支付";
      button.addEventListener("click", () => showPaymentForOrder(order, paymentDetails(order)));
      row.append(button);
    } else if (order.status === "pending") {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "button button-quiet";
      button.textContent = "刷新";
      button.addEventListener("click", () => loadOrders());
      row.append(button);
    }
    return row;
  }));
}

function orderStatus(order) {
  if (order.status === "pending" && !paymentDetails(order)) return "订单处理中";
  return ({ pending: "待支付", paid: "已支付", closed: "已关闭", refunded: "已退款", refunding: "退款中" })[order.status] || String(order.status || "待确认");
}

function renderUsage() {
  const usage = state.usage || {};
  const topicsUsed = usage.topicsUsed === null || usage.topicsUsed === undefined ? state.topics.length : usage.topicsUsed;
  const topicsLimit = usage.topicsLimit ?? state.limits?.maxTopics ?? defaultLimits().maxTopics;
  const runsUsed = usage.runsUsed ?? 0;
  const runsLimit = usage.runsLimit ?? state.limits?.dailyRuns ?? defaultLimits().dailyRuns;
  renderMetric(el.topicsUsageValue, el.topicsUsageMeta, el.topicsUsageProgress, topicsUsed, topicsLimit, "个主题");
  renderMetric(el.runsUsageValue, el.runsUsageMeta, el.runsUsageProgress, runsUsed, runsLimit, "次运行");
  el.usageReset.textContent = usage.resetAt ? `${formatReset(usage.resetAt)}重置` : "北京时间每日 00:00 重置";
}

function renderMetric(valueEl, metaEl, progressEl, used, limit, unit) {
  const current = Math.max(0, Number(used) || 0);
  const max = Math.max(1, Number(limit) || 1);
  valueEl.textContent = `${Math.floor(current)}/${Math.floor(max)}`;
  metaEl.textContent = current >= max ? "已达到上限" : `剩余 ${Math.max(0, Math.floor(max - current))} ${unit}`;
  progressEl.querySelector("span").style.width = `${Math.min(100, current / max * 100)}%`;
  progressEl.classList.toggle("is-limit", current >= max);
  progressEl.setAttribute("aria-valuemax", String(max));
  progressEl.setAttribute("aria-valuenow", String(Math.min(current, max)));
}

// ---------------------------------------------------------------------------
// YPay orders and QR display
// ---------------------------------------------------------------------------

async function createProOrder(button) {
  if (button?.disabled || !state.user) return;
  const epoch = state.sessionEpoch;
  const userId = state.user.id;
  const idleLabel = button.textContent;
  setBusy(button, true, "创建中…");
  try {
    const data = await api("/api/orders", { method: "POST", body: JSON.stringify({ plan_code: "pro" }) });
    if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
    const order = data?.order || data;
    const payment = data?.payment || data?.ypay_payment || paymentDetails(order);
    if (!order?.out_trade_no) throw new Error("订单信息不完整");
    state.orders = [order, ...state.orders.filter((item) => item.out_trade_no !== order.out_trade_no)];
    renderOrders();
    // A fast webhook can mark the order paid before the create response is
    // stored locally. In that case there is no QR left to show: refresh the
    // account immediately and keep the checkout flow visually quiet.
    if (order.status === "paid") {
      await refreshAccount({ quiet: true });
      if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
      await loadTopics();
      if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
      showToast("支付成功，Pro 已开通", "ok");
      return;
    }
    showPaymentForOrder(order, payment);
    showToast("订单已创建，请使用微信扫码支付", "ok");
  } catch (error) {
    if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
    // The local order is committed before the YPay request.  A provider
    // timeout can therefore leave a pending order even though this request
    // returns an error; refresh the short order list so the user sees it.
    try { await loadOrders(); } catch { /* keep the original checkout error */ }
    if (epoch !== state.sessionEpoch || !state.user || state.user.id !== userId) return;
    showToast(error.message || "订单创建失败，请稍后重试", "error");
  } finally {
    setBusy(button, false, idleLabel);
  }
}

function paymentDetails(order) {
  const details = order?.payment || order?.metadata?.ypay_payment || order?.metadata?.payment || order;
  if (!details || typeof details !== "object") return null;
  const codeUrl = details.code_url || details.native_code || details.native_code_url || details.pay_url;
  const qrUrl = details.qr_url || details.qr_image_url || details.qr_image || details.qrcode;
  if (!codeUrl && !qrUrl) return null;
  return { ...details, code_url: codeUrl, qr_url: qrUrl };
}

function showPaymentForOrder(order, details) {
  if (!order) return;
  stopPaymentPolling();
  state.payment = { order, details: details || {} };
  renderPayment(order, state.payment.details);
  el.paymentPanel.classList.remove("hidden");
  startPaymentPolling(order.out_trade_no);
  window.requestAnimationFrame(() => el.paymentPanel.scrollIntoView({ behavior: preferredScrollBehavior(), block: "nearest" }));
}

function renderPayment(order, details) {
  const codeUrl = safePaymentUrl(details?.code_url);
  // Native YPay responses contain a `code_url`, so the server can render a
  // same-origin SVG without shipping a QR library or trusting a remote image.
  const qrEndpoint = order?.out_trade_no
    ? `/api/orders/${encodeURIComponent(order.out_trade_no)}/qr.svg`
    : "";
  const fallbackQr = safeImageUrl(details?.qr_url);
  const amount = Number(order.amount_fen ?? details?.amount ?? 9900);
  el.paymentContent.replaceChildren();
  const card = document.createElement("article");
  card.className = "me-payment-card";
  const qr = document.createElement("div");
  qr.className = "me-payment-qr";
  qr.setAttribute("aria-label", "微信支付二维码");
  if (qrEndpoint || fallbackQr) {
    const image = document.createElement("img");
    image.src = qrEndpoint || fallbackQr;
    image.alt = "微信支付二维码";
    let fallbackUsed = !qrEndpoint;
    image.addEventListener("error", () => {
      if (!fallbackUsed && fallbackQr) {
        fallbackUsed = true;
        image.src = fallbackQr;
      } else {
        renderCodeQr(qr, codeUrl);
      }
    });
    qr.append(image);
  } else {
    renderCodeQr(qr, codeUrl);
  }
  const info = document.createElement("div");
  info.className = "me-payment-info";
  const heading = document.createElement("strong");
  heading.textContent = "微信扫码支付";
  const amountNode = document.createElement("span");
  amountNode.className = "me-payment-amount";
  amountNode.textContent = `¥${(Number.isFinite(amount) ? amount : 9900) / 100}`;
  const status = document.createElement("span");
  status.id = "paymentStatus";
  status.className = "me-payment-status";
  status.textContent = "等待支付结果";
  info.append(heading, amountNode, status);
  if (codeUrl) {
    const link = document.createElement("a");
    link.className = "me-payment-link";
    link.href = codeUrl;
    link.textContent = "无法扫码时打开支付链接";
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    info.append(link);
    const code = document.createElement("code");
    code.className = "me-payment-code";
    code.textContent = codeUrl;
    info.append(code);
  }
  card.append(qr, info);
  el.paymentContent.append(card);
}

function renderCodeQr(container, codeUrl) {
  container.replaceChildren();
  if (!codeUrl) {
    container.textContent = "二维码暂不可用";
    return;
  }
  const note = document.createElement("span");
  note.textContent = "二维码暂不可用，请点击下方链接完成支付";
  container.append(note);
}

function startPaymentPolling(outTradeNo) {
  if (!outTradeNo) return;
  const epoch = state.sessionEpoch;
  const userId = state.user?.id;
  const isCurrent = () => epoch === state.sessionEpoch
    && !!state.user
    && state.user.id === userId
    && state.payment?.order?.out_trade_no === outTradeNo;
  stopPaymentPolling();
  state.paymentStartedAt = Date.now();
  state.paymentAttempts = 0;
  const poll = async () => {
    if (!isCurrent()) return;
    state.paymentAttempts += 1;
    if (Date.now() - state.paymentStartedAt > PAYMENT_POLL_MAX_MS) {
      updatePaymentStatus("仍在等待支付结果，可稍后刷新订单。", "error");
      return;
    }
    try {
      const order = await fetchOrder(outTradeNo);
      if (!isCurrent()) return;
      if (!order) {
        updatePaymentStatus("订单暂不可用，请刷新订单列表。", "error");
        return;
      }
      state.payment.order = order;
      state.orders = [order, ...state.orders.filter((item) => item.out_trade_no !== outTradeNo)];
      renderOrders();
      if (order.status === "paid") {
        updatePaymentStatus("支付成功，Pro 已开通。", "paid");
        stopPaymentPolling();
        await refreshAccount({ quiet: true });
        if (!isCurrent()) return;
        await loadTopics();
        if (!isCurrent()) return;
        showToast("支付成功，Pro 已开通", "ok");
        return;
      }
      if (!["pending", "processing"].includes(String(order.status))) {
        updatePaymentStatus(`订单状态：${orderStatus(order)}`, "error");
        return;
      }
      state.paymentTimer = window.setTimeout(poll, Math.min(30_000, 1_800 + state.paymentAttempts * 300));
    } catch (error) {
      if (!isCurrent()) return;
      if (error.status === 404) {
        state.paymentTimer = window.setTimeout(poll, 5_000);
      } else {
        state.paymentTimer = window.setTimeout(poll, Math.min(30_000, 2_000 * 2 ** Math.min(state.paymentAttempts, 4)));
      }
    }
  };
  state.paymentTimer = window.setTimeout(poll, 1_800);
}

async function fetchOrder(outTradeNo) {
  try {
    const data = await api(`/api/orders/${encodeURIComponent(outTradeNo)}`);
    return data?.order || data;
  } catch (error) {
    if (![404, 405].includes(error.status)) throw error;
    const data = await api("/api/orders");
    return extractList(data, "orders").find((order) => order.out_trade_no === outTradeNo) || null;
  }
}

function updatePaymentStatus(text, kind = "") {
  const node = $("#paymentStatus");
  if (!node) return;
  node.textContent = text;
  node.classList.toggle("is-paid", kind === "paid");
  node.classList.toggle("is-error", kind === "error");
}

function stopPaymentPolling() {
  if (state.paymentTimer) window.clearTimeout(state.paymentTimer);
  state.paymentTimer = null;
}

function hidePayment() {
  stopPaymentPolling();
  state.payment = null;
  el.paymentPanel.classList.add("hidden");
  el.paymentContent.replaceChildren();
}

// ---------------------------------------------------------------------------
// Rendering and dialogs
// ---------------------------------------------------------------------------

function renderAll() {
  if (!state.user) return;
  renderTopicList();
  renderPlans();
  renderOrders();
  renderUsage();
}

function openConfirm(title, message, action) {
  el.confirmTitle.textContent = title;
  el.confirmMessage.textContent = message;
  state.confirmAction = action;
  if (!el.confirmDialog.open) el.confirmDialog.showModal();
  document.body.classList.add("dialog-open");
  window.requestAnimationFrame(() => el.confirmActionButton.focus());
}

async function runConfirmedAction() {
  if (!state.confirmAction) return;
  const action = state.confirmAction;
  setBusy(el.confirmActionButton, true, "删除中…");
  try {
    await action();
    closeDialog(el.confirmDialog);
  } catch (error) {
    showToast(topicErrorMessage(error), "error");
  } finally {
    setBusy(el.confirmActionButton, false, "确认删除");
  }
}

function closeDialog(dialog) {
  if (dialog?.open) dialog.close();
  if (dialog === el.confirmDialog) state.confirmAction = null;
  if (![el.topicDialog, el.confirmDialog].some((item) => item?.open)) document.body.classList.remove("dialog-open");
}

function bindDialogDismissal(dialog) {
  let backdropPress = false;
  const outside = (event) => {
    const rect = dialog.getBoundingClientRect();
    return event.target === dialog && !(event.clientX >= rect.left && event.clientX <= rect.right && event.clientY >= rect.top && event.clientY <= rect.bottom);
  };
  dialog.addEventListener("pointerdown", (event) => { backdropPress = outside(event); });
  dialog.addEventListener("click", (event) => {
    if (backdropPress && outside(event)) closeDialog(dialog);
    backdropPress = false;
  });
  dialog.addEventListener("cancel", (event) => {
    if (dialog.querySelector("[aria-busy='true']")) event.preventDefault();
  });
  dialog.addEventListener("close", () => closeDialog(dialog));
}

function setBusy(button, busy, label) {
  if (!button) return;
  button.disabled = busy;
  button.setAttribute("aria-busy", String(busy));
  if (label !== undefined) button.textContent = label;
}

function showToast(message, type = "success") {
  if (!el.toastRegion) return;
  const toast = document.createElement("div");
  toast.className = `toast ${type === "ok" ? "success" : type}`;
  toast.setAttribute("role", type === "error" ? "alert" : "status");
  toast.textContent = String(message || "操作完成");
  el.toastRegion.replaceChildren(toast);
  window.setTimeout(() => {
    toast.classList.add("is-leaving");
    window.setTimeout(() => toast.remove(), 240);
  }, type === "error" ? 7_000 : 4_000);
}

function extractList(data, key) {
  if (Array.isArray(data)) return data;
  if (Array.isArray(data?.[key])) return data[key];
  if (Array.isArray(data?.data)) return data.data;
  return [];
}

function findFallbackPlan(code) {
  return FALLBACK_PLANS.find((plan) => plan.code === code);
}

function safeLoginUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(String(value), window.location.href);
    return ["http:", "https:"].includes(url.protocol) && url.origin === window.location.origin ? url.href : "";
  } catch { return ""; }
}

function safePaymentUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(String(value), window.location.href);
    return ["http:", "https:", "weixin:"].includes(url.protocol) ? url.href : "";
  } catch { return ""; }
}

function safeImageUrl(value) {
  if (!value) return "";
  const text = String(value);
  if (/^data:image\/(?:png|jpeg|webp);base64,/i.test(text)) return text;
  try {
    const url = new URL(text, window.location.href);
    return ["http:", "https:"].includes(url.protocol) ? url.href : "";
  } catch { return ""; }
}

function safeArticleUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(String(value), window.location.href);
    return ["http:", "https:"].includes(url.protocol) && url.hostname ? url.href : "";
  } catch { return ""; }
}

function takeLoginToken() {
  const url = new URL(window.location.href);
  const fragment = new URLSearchParams(url.hash.replace(/^#/u, ""));
  const token = fragment.get("login_token") || url.searchParams.get("login_token");
  if (!token) return null;
  fragment.delete("login_token");
  url.searchParams.delete("login_token");
  url.hash = fragment.toString() ? `#${fragment.toString()}` : "";
  window.history.replaceState({}, "", `${url.pathname}${url.search}${url.hash}`);
  return token;
}

function formatDate(value) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString("zh-CN", { timeZone: state.timezone, year: "numeric", month: "2-digit", day: "2-digit" });
}

function shortDateTime(value) {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString("zh-CN", { timeZone: state.timezone, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
}

function isTimezone(value) {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

function formatReset(value) {
  return shortDateTime(value) || "明日 00:00";
}

function resetText() {
  return state.usage?.resetAt ? `${formatReset(state.usage.resetAt)} ` : "明日 ";
}

function preferredScrollBehavior() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
}
