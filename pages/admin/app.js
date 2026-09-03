"use strict";

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"];
const RUN_POLL_MS = 5000;
const STATUS_REFRESH_MS = 60000;
const USD_PER_TICK = 1e-10;
const DEFAULT_TIMEZONE = "Asia/Shanghai";

const state = {
  user: null,
  topics: [],
  running: [],
  selected: null,
  creating: false,
  runs: [],
  items: [],
  loginInFlight: false,
  confirmAction: null,
  timezone: DEFAULT_TIMEZONE,
  sessionEpoch: 0,
  // Guards against a slow response for one topic landing after the user picked another.
  epoch: 0,
  poll: null,
};

const el = Object.fromEntries(
  [
    "boot", "authView", "appView", "loginForm", "email", "mailSent", "sentEmail", "devLoginLink", "sendAgain",
    "statusLine", "accountEmail", "logoutButton", "newTopicButton", "topicList", "emptyState", "topicPanel",
    "topicForm", "formTitle", "runButton", "copyUrlButton", "deleteTopicButton", "topicId", "topicName",
    "topicNameEn", "topicDescription", "topicBrief", "topicInterval", "windowStart", "windowEnd", "weekdays",
    "topicModel", "topicEffort", "topicMaxItems", "topicLookback", "topicCap", "topicSort", "saveButton",
    "topicEnabled", "scheduleInfo", "detailSections", "runsMeta", "runsBody", "itemsDay", "itemsMeta", "itemList",
    "confirmDialog", "confirmTitle", "confirmMessage", "confirmActionButton", "toastRegion",
  ].map((id) => [id, $(`#${id}`)]),
);

document.addEventListener("DOMContentLoaded", initialize);

async function initialize() {
  buildStaticInputs();
  bindEvents();
  const loginToken = takeLoginToken();
  try {
    if (loginToken) await finishLogin(loginToken);
    else await restoreSession();
  } catch (error) {
    showAuth();
    showToast(error.message || "暂时无法打开管理页", "error");
  } finally {
    window.requestAnimationFrame(() => el.boot.classList.add("is-ready"));
  }
}

function buildStaticInputs() {
  const option = (value) => Object.assign(document.createElement("option"), { value, textContent: `${value}:00` });
  el.windowStart.replaceChildren(...Array.from({ length: 24 }, (_, hour) => option(hour)));
  el.windowEnd.replaceChildren(...Array.from({ length: 24 }, (_, hour) => option(hour + 1)));
  el.weekdays.replaceChildren(...WEEKDAYS.map((label, index) => {
    const wrap = document.createElement("label");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.value = String(index + 1);
    wrap.append(box, `周${label}`);
    return wrap;
  }));
  el.itemsDay.value = dateInTimezone(new Date());
  el.itemsDay.dataset.userSelected = "false";
}

function bindEvents() {
  el.loginForm.addEventListener("submit", requestLogin);
  el.sendAgain.addEventListener("click", () => {
    resetLoginForm();
    el.email.focus();
  });
  el.logoutButton.addEventListener("click", logout);
  el.newTopicButton.addEventListener("click", newTopic);
  el.topicForm.addEventListener("submit", saveTopic);
  el.runButton.addEventListener("click", runNow);
  el.copyUrlButton.addEventListener("click", copyPublicUrl);
  el.deleteTopicButton.addEventListener("click", confirmDeleteTopic);
  el.itemsDay.addEventListener("change", () => {
    el.itemsDay.dataset.userSelected = "true";
    if (state.selected) reloadItems();
  });
  el.confirmActionButton.addEventListener("click", runConfirmedAction);
  $$("[data-close-dialog]").forEach((button) => {
    button.addEventListener("click", () => closeDialog(button.closest("dialog")));
  });
  $$("dialog").forEach(bindDialogDismissal);
  window.addEventListener("hashchange", handleLoginTokenFragment);
  window.setInterval(() => {
    if (state.user && document.visibilityState === "visible") refreshStatus();
  }, STATUS_REFRESH_MS);
}

// ---------------------------------------------------------------------------
// Transport and session
// ---------------------------------------------------------------------------

async function api(path, options = {}) {
  let response;
  try {
    response = await fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      ...options,
      headers: {
        ...(options.body ? { "Content-Type": "application/json" } : {}),
        ...(options.headers || {}),
      },
    });
  } catch {
    throw new Error("网络连接失败，请检查网络后重试");
  }
  const payload = (response.headers.get("content-type") || "").includes("application/json")
    ? await response.json().catch(() => null)
    : null;
  if (!response.ok) {
    const error = new Error(payload?.error?.message || `请求失败（${response.status}）`);
    error.status = response.status;
    if (state.user && (response.status === 401 || response.status === 403)) {
      showAuth();
      showToast("登录状态已失效，请重新登录", "error");
    }
    throw error;
  }
  return payload;
}

async function restoreSession() {
  try {
    const data = await api("/api/me");
    await enterApp(data?.user);
  } catch (error) {
    if (error.status !== 401 && error.status !== 403) throw error;
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
    const data = await api("/api/auth/email/start", { method: "POST", body: JSON.stringify({ email }) });
    el.sentEmail.textContent = data?.email || email;
    el.loginForm.classList.add("hidden");
    el.mailSent.classList.remove("hidden");
    const loginUrl = safeLoginUrl(data?.login_url);
    el.devLoginLink.classList.toggle("hidden", !loginUrl);
    if (loginUrl) el.devLoginLink.href = loginUrl;
    else el.devLoginLink.removeAttribute("href");
    window.requestAnimationFrame(() => (loginUrl ? el.devLoginLink : el.sendAgain).focus());
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
    const data = await api("/api/auth/email/consume", { method: "POST", body: JSON.stringify({ token }) });
    await enterApp(data?.user);
  } catch (error) {
    showAuth();
    throw new Error(error.message || "登录链接已失效，请重新发送");
  } finally {
    state.loginInFlight = false;
  }
}

async function enterApp(user) {
  if (!user?.email) throw new Error("登录状态无效，请重新登录");
  if (String(user.role || "user").toLowerCase() !== "admin") {
    // A regular account has its own, deliberately smaller console.
    window.location.replace("/me/");
    return;
  }
  state.sessionEpoch += 1;
  state.user = user;
  el.authView.classList.add("hidden");
  el.appView.classList.remove("hidden");
  el.accountEmail.textContent = user.email;
  try {
    // Load the service timezone before choosing the initial item date.
    await refreshStatus();
    await loadTopics();
  } catch (error) {
    showToast(error.message, "error");
  }
}

function showAuth() {
  state.sessionEpoch += 1;
  state.user = null;
  state.topics = [];
  state.selected = null;
  state.epoch += 1;
  window.clearInterval(state.poll);
  state.poll = null;
  closeDialog(el.confirmDialog);
  el.topicList.replaceChildren();
  el.accountEmail.textContent = "";
  el.statusLine.textContent = "";
  showTopic(null);
  el.appView.classList.add("hidden");
  el.authView.classList.remove("hidden");
  resetLoginForm();
  window.requestAnimationFrame(() => el.email.focus());
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
    await api("/api/auth/logout", { method: "POST" });
    showAuth();
  } catch (error) {
    showToast(error.message || "暂时无法退出，请稍后重试", "error");
  } finally {
    setBusy(el.logoutButton, false, "退出");
  }
}

// ---------------------------------------------------------------------------
// Topics
// ---------------------------------------------------------------------------

async function loadTopics(selectId = state.selected?.id) {
  const sessionEpoch = state.sessionEpoch;
  const selectionEpoch = state.epoch;
  const topics = (await api("/api/admin/topics")) || [];
  if (sessionEpoch !== state.sessionEpoch || !state.user) return;
  state.topics = topics;
  // A user may have selected another topic while the request was in flight.
  // Keep that newer choice instead of letting the older request jump back.
  const selectionChanged = selectionEpoch !== state.epoch;
  const currentId = state.selected?.id;
  const targetId = selectionChanged ? currentId : selectId;
  const selected = state.topics.find((topic) => topic.id === targetId)
    || (selectionChanged ? null : state.topics[0]);
  renderTopics();
  if (selectionChanged) {
    if (selected) {
      state.selected = selected;
      if (!state.creating) showTopic(selected);
    } else if (!state.creating) {
      state.selected = null;
      showTopic(null);
    }
    return;
  }
  if (selected) await selectTopic(selected.id);
  else showTopic(null);
}

async function refreshStatus() {
  const sessionEpoch = state.sessionEpoch;
  try {
    const status = await api("/api/admin/status");
    if (sessionEpoch !== state.sessionEpoch || !state.user) return;
    if (status.timezone && isTimezone(status.timezone)) {
      const changed = state.timezone !== status.timezone;
      state.timezone = status.timezone;
      if (changed && el.itemsDay.dataset.userSelected !== "true") {
        el.itemsDay.value = dateInTimezone(new Date());
      }
    }
    state.running = status.running;
    el.statusLine.textContent = `在飞 ${status.running.length} · 今日 ${status.today.runs} 次 · ${usd(status.today.cost_ticks, 2)}`;
    renderTopics();
  } catch {
    // Decorative; the next refresh will retry.
  }
}

function renderTopics() {
  const entries = state.topics.map((topic) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "entry";
    button.title = topic.id;
    if (!state.creating && state.selected?.id === topic.id) button.setAttribute("aria-current", "true");
    const dot = document.createElement("span");
    dot.className = `dot ${state.running.includes(topic.id) ? "running" : topic.enabled ? topic.last_status || "" : ""}`;
    const name = document.createElement("span");
    name.textContent = topic.name;
    const meta = document.createElement("span");
    meta.className = "entry-meta";
    meta.textContent = !topic.enabled ? "已停用"
      : state.running.includes(topic.id) ? "抓取中…"
        : topic.next_run_at ? `下次 ${shortDateTime(topic.next_run_at)}` : "未排程";
    button.append(dot, name, meta);
    button.addEventListener("click", () => {
      selectTopic(topic.id).catch((error) => showToast(error.message, "error"));
    });
    return button;
  });
  if (!entries.length) {
    const empty = document.createElement("p");
    empty.className = "muted small";
    empty.textContent = "还没有主题";
    entries.push(empty);
  }
  el.topicList.replaceChildren(...entries);
}

async function selectTopic(id) {
  const topic = state.topics.find((entry) => entry.id === id);
  if (!topic) return showTopic(null);
  const epoch = ++state.epoch;
  state.selected = topic;
  state.creating = false;
  renderTopics();
  showTopic(topic);
  const [runs, items] = await Promise.all([loadRuns(topic.id), loadItems(topic.id)]);
  if (epoch !== state.epoch) return;
  state.runs = runs;
  state.items = items;
  renderRuns();
  renderItems();
}

function newTopic() {
  state.creating = true;
  state.selected = null;
  state.epoch += 1;
  renderTopics();
  showTopic(null, true);
  el.topicId.focus();
}

function showTopic(topic, creating = false) {
  const visible = Boolean(topic) || creating;
  el.emptyState.classList.toggle("hidden", visible);
  el.topicPanel.classList.toggle("hidden", !visible);
  el.detailSections.classList.toggle("hidden", !topic);
  [el.runButton, el.copyUrlButton, el.deleteTopicButton].forEach((button) => button.classList.toggle("hidden", !topic));
  if (!visible) return;
  fillForm(topic);
}

function fillForm(topic) {
  const value = (field, fallback) => (topic ? topic[field] : fallback);
  el.formTitle.textContent = topic ? topic.name : "新建主题";
  el.topicId.value = value("id", "");
  el.topicId.readOnly = Boolean(topic);
  el.topicName.value = value("name", "");
  el.topicNameEn.value = value("name_en", "");
  el.topicDescription.value = value("description", "");
  el.topicBrief.value = value("brief", "");
  el.topicInterval.value = value("interval_minutes", 60);
  el.windowStart.value = String(value("window_start", 6));
  el.windowEnd.value = String(value("window_end", 24));
  const weekdays = value("weekdays", [1, 2, 3, 4, 5]);
  $$("input", el.weekdays).forEach((box) => {
    box.checked = weekdays.includes(Number(box.value));
  });
  el.topicModel.value = value("model", "");
  el.topicEffort.value = value("reasoning_effort", "low");
  el.topicMaxItems.value = value("max_items", 8);
  el.topicLookback.value = value("lookback_hours", 24);
  el.topicCap.value = value("daily_run_cap", 24);
  el.topicSort.value = value("sort_weight", 0);
  el.topicEnabled.checked = value("enabled", true);
  el.scheduleInfo.textContent = topic ? scheduleLabel(topic) : "";
}

function scheduleLabel(topic) {
  const parts = [];
  if (topic.last_run_at) parts.push(`上次 ${shortDateTime(topic.last_run_at)} ${topic.last_status || ""}`.trim());
  if (topic.enabled) parts.push(topic.next_run_at ? `下次 ${shortDateTime(topic.next_run_at)}` : "未排程");
  if (topic.consecutive_failures) parts.push(`连续失败 ${topic.consecutive_failures} 次`);
  return parts.join(" · ");
}

function formPayload() {
  return {
    name: el.topicName.value,
    name_en: el.topicNameEn.value,
    description: el.topicDescription.value,
    brief: el.topicBrief.value,
    enabled: el.topicEnabled.checked,
    interval_minutes: Number(el.topicInterval.value),
    window_start: Number(el.windowStart.value),
    window_end: Number(el.windowEnd.value),
    weekdays: $$("input:checked", el.weekdays).map((box) => Number(box.value)),
    model: el.topicModel.value,
    reasoning_effort: el.topicEffort.value,
    max_items: Number(el.topicMaxItems.value),
    lookback_hours: Number(el.topicLookback.value),
    daily_run_cap: Number(el.topicCap.value),
    sort_weight: Number(el.topicSort.value),
  };
}

async function saveTopic(event) {
  event.preventDefault();
  const creating = state.creating;
  const id = creating ? el.topicId.value.trim() : state.selected?.id;
  if (!id) return;
  setBusy(el.saveButton, true, "保存中…");
  try {
    const saved = creating
      ? await api("/api/admin/topics", { method: "POST", body: JSON.stringify({ id, ...formPayload() }) })
      : await api(`/api/admin/topics/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(formPayload()) });
    showToast(creating ? "主题已创建。" : "已保存。");
    state.creating = false;
    await loadTopics(saved.id);
  } catch (error) {
    showToast(error.message, "error");
  } finally {
    setBusy(el.saveButton, false, "保存");
  }
}

function confirmDeleteTopic() {
  const topic = state.selected;
  if (!topic) return;
  openConfirm("删除这个主题？", `“${topic.name}”及其全部运行记录和条目都会永久删除。`, async () => {
    await api(`/api/admin/topics/${encodeURIComponent(topic.id)}`, { method: "DELETE" });
    state.selected = null;
    await loadTopics();
    showToast("主题已删除。");
  });
}

async function copyPublicUrl() {
  if (!state.selected) return;
  try {
    await copyText(`${location.origin}/digest?topic=${encodeURIComponent(state.selected.id)}`);
    showToast("地址已复制。");
  } catch {
    showToast("复制失败，请手动复制。", "error");
  }
}

async function runNow() {
  const topic = state.selected;
  if (!topic) return;
  setBusy(el.runButton, true, "已提交…");
  try {
    await api(`/api/admin/topics/${encodeURIComponent(topic.id)}/run`, { method: "POST" });
    showToast("抓取已开始，通常需要 1 到 2 分钟。");
    await refreshStatus();
    pollRuns(topic.id);
  } catch (error) {
    showToast(error.message, "error");
  } finally {
    setBusy(el.runButton, false, "立即抓取");
  }
}

// Watches the run history until the newest run leaves `running`, then reloads everything.
function pollRuns(topicId) {
  window.clearInterval(state.poll);
  let remaining = 90;
  let timer;
  const stop = () => {
    window.clearInterval(timer);
    if (state.poll === timer) state.poll = null;
  };
  const sessionEpoch = state.sessionEpoch;
  timer = window.setInterval(async () => {
    if (sessionEpoch !== state.sessionEpoch || !state.user || state.selected?.id !== topicId || remaining-- <= 0) return stop();
    try {
      const runs = await loadRuns(topicId);
      if (sessionEpoch !== state.sessionEpoch || !state.user || state.selected?.id !== topicId) return stop();
      state.runs = runs;
      renderRuns();
      if (state.runs[0] && state.runs[0].status !== "running") {
        stop();
        await loadTopics(topicId);
        if (sessionEpoch !== state.sessionEpoch || !state.user || state.selected?.id !== topicId) return;
        await refreshStatus();
      }
    } catch {
      // Keep polling; the next tick may succeed.
    }
  }, RUN_POLL_MS);
  state.poll = timer;
}

// ---------------------------------------------------------------------------
// Runs and items
// ---------------------------------------------------------------------------

function loadRuns(topicId) {
  return api(`/api/admin/topics/${encodeURIComponent(topicId)}/runs?limit=50`).then((list) => list || []);
}

function loadItems(topicId) {
  return api(`/api/admin/topics/${encodeURIComponent(topicId)}/items?day=${encodeURIComponent(el.itemsDay.value)}`)
    .then((list) => list || []);
}

async function reloadItems() {
  const topic = state.selected;
  const epoch = state.epoch;
  try {
    const items = await loadItems(topic.id);
    if (epoch !== state.epoch) return;
    state.items = items;
    renderItems();
  } catch (error) {
    showToast(error.message, "error");
  }
}

function renderRuns() {
  el.runsMeta.textContent = state.runs.length ? `最近 ${state.runs.length} 次` : "还没有运行";
  el.runsBody.replaceChildren(...state.runs.map((run) => {
    const row = document.createElement("tr");
    const pill = document.createElement("span");
    pill.className = `status-pill ${run.status}`;
    pill.textContent = run.status;
    const duration = run.finished_at
      ? `${Math.round((Date.parse(run.finished_at) - Date.parse(run.started_at)) / 1000)}s`
      : "…";
    const cells = [
      `${shortDateTime(run.started_at)}${run.triggered_by === "manual" ? " 手动" : ""}`,
      pill,
      `${run.returned_count} / ${run.new_count} / ${run.dup_count} / ${run.dropped_count}`,
      `${run.input_tokens.toLocaleString()} / ${run.output_tokens.toLocaleString()}`,
      `${run.web_search_calls} / ${run.x_search_calls}`,
      usd(run.cost_ticks, 3),
      duration,
      run.error || "",
    ].map((content, index) => {
      const cell = document.createElement("td");
      if (index === 7) {
        cell.className = "error-cell";
        cell.title = content;
      }
      cell.append(content);
      return cell;
    });
    row.append(...cells);
    return row;
  }));
}

function renderItems() {
  el.itemsMeta.textContent = `${state.items.length} 条`;
  if (!state.items.length) {
    const empty = document.createElement("p");
    empty.className = "muted";
    empty.textContent = "这一天没有条目。";
    el.itemList.replaceChildren(empty);
    return;
  }
  el.itemList.replaceChildren(...state.items.map(itemRow));
}

function itemRow(item) {
  const row = document.createElement("article");
  row.className = "item-row";
  row.classList.toggle("is-pinned", item.pinned);
  row.classList.toggle("is-hidden", item.hidden);

  const body = document.createElement("div");
  body.className = "item-row-body";
  const link = document.createElement("a");
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.textContent = item.title;
  const summary = document.createElement("p");
  summary.textContent = item.summary;
  const meta = document.createElement("small");
  meta.className = "muted";
  meta.textContent = [
    item.source,
    item.published_at ? shortDateTime(item.published_at) : "",
    `重要度 ${item.importance}`,
    `出现 ${item.seen_count} 次`,
    ...(item.pinned ? ["已置顶"] : []),
    ...(item.hidden ? ["已隐藏"] : []),
  ].filter(Boolean).join(" · ");
  body.append(link, summary, meta);

  const actions = document.createElement("div");
  actions.className = "row-actions";
  actions.append(
    actionButton(item.pinned ? "取消置顶" : "置顶", () => patchItem(item, { pinned: !item.pinned })),
    actionButton(item.hidden ? "显示" : "隐藏", () => patchItem(item, { hidden: !item.hidden })),
    actionButton("删除", () => confirmDeleteItem(item)),
  );
  row.append(body, actions);
  return row;
}

async function patchItem(item, changes) {
  const epoch = state.epoch;
  try {
    const updated = await api(`/api/admin/items/${encodeURIComponent(item.id)}`, { method: "PATCH", body: JSON.stringify(changes) });
    if (epoch !== state.epoch) return;
    state.items = state.items.map((entry) => (entry.id === updated.id ? updated : entry));
    renderItems();
  } catch (error) {
    showToast(error.message, "error");
  }
}

function confirmDeleteItem(item) {
  const epoch = state.epoch;
  openConfirm("删除这条内容？", "删除后不会再出现在公开页面。", async () => {
    await api(`/api/admin/items/${encodeURIComponent(item.id)}`, { method: "DELETE" });
    showToast("已删除。");
    if (epoch !== state.epoch) return;
    state.items = state.items.filter((entry) => entry.id !== item.id);
    renderItems();
  });
}

function actionButton(label, onClick) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "button button-quiet button-small";
  button.textContent = label;
  button.addEventListener("click", onClick);
  return button;
}

// ---------------------------------------------------------------------------
// Dialogs, toasts, helpers
// ---------------------------------------------------------------------------

function openConfirm(title, message, action) {
  el.confirmTitle.textContent = title;
  el.confirmMessage.textContent = message;
  state.confirmAction = action;
  if (!el.confirmDialog.open) el.confirmDialog.showModal();
  document.body.classList.add("dialog-open");
  window.requestAnimationFrame(() => el.confirmActionButton.focus());
}

async function runConfirmedAction() {
  const action = state.confirmAction;
  if (!action) return;
  setBusy(el.confirmActionButton, true, "删除中…");
  try {
    await action();
    closeDialog(el.confirmDialog);
  } catch (error) {
    showToast(error.message, "error");
  } finally {
    setBusy(el.confirmActionButton, false, "确认删除");
  }
}

function closeDialog(dialog) {
  if (dialog?.open) dialog.close();
  state.confirmAction = null;
  document.body.classList.remove("dialog-open");
}

function bindDialogDismissal(dialog) {
  const busy = () => Boolean($('[aria-busy="true"]', dialog));
  const onBackdrop = (event) => {
    const rect = dialog.getBoundingClientRect();
    return event.target === dialog
      && !(event.clientX >= rect.left && event.clientX <= rect.right && event.clientY >= rect.top && event.clientY <= rect.bottom);
  };
  let backdropPress = false;
  dialog.addEventListener("pointerdown", (event) => {
    backdropPress = onBackdrop(event);
  });
  dialog.addEventListener("click", (event) => {
    if (backdropPress && onBackdrop(event) && !busy()) closeDialog(dialog);
    backdropPress = false;
  });
  dialog.addEventListener("cancel", (event) => {
    if (busy()) event.preventDefault();
  });
  dialog.addEventListener("close", () => closeDialog(dialog));
}

function setBusy(button, busy, label) {
  if (!button) return;
  button.disabled = busy;
  button.setAttribute("aria-busy", String(busy));
  const labelNode = $("span", button) || button;
  if (label !== undefined) labelNode.textContent = label;
}

function showToast(message, type = "success") {
  const toast = document.createElement("div");
  toast.className = `toast ${type}`;
  toast.setAttribute("role", type === "error" ? "alert" : "status");
  toast.textContent = String(message || "操作完成");
  el.toastRegion.replaceChildren(toast);
  window.setTimeout(() => {
    toast.classList.add("is-leaving");
    window.setTimeout(() => toast.remove(), 240);
  }, type === "error" ? 7000 : 4000);
}

async function copyText(text) {
  if (navigator.clipboard?.writeText && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Fall through to the legacy path for restricted browser contexts.
    }
  }
  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "");
  input.className = "clipboard-proxy";
  document.body.appendChild(input);
  let copied = false;
  try {
    input.select();
    copied = document.execCommand("copy");
  } finally {
    input.remove();
  }
  if (!copied) throw new Error("copy failed");
}

const usd = (ticks, digits) => `$${(ticks * USD_PER_TICK).toFixed(digits)}`;

function localDate(value) {
  const date = new Date(value);
  const parts = new Intl.DateTimeFormat("en-GB", {
    timeZone: state.timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).formatToParts(date).reduce((result, part) => {
    result[part.type] = part.value;
    return result;
  }, {});
  return `${parts.year}-${parts.month}-${parts.day}T${parts.hour === "24" ? "00" : parts.hour}:${parts.minute}`;
}

const shortDateTime = (value) => localDate(value).replace("T", " ").slice(5);

function dateInTimezone(value) {
  return localDate(value).slice(0, 10);
}

function isTimezone(value) {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

function safeLoginUrl(value) {
  if (!value) return "";
  try {
    const url = new URL(String(value), window.location.href);
    return ["http:", "https:"].includes(url.protocol) && url.origin === window.location.origin ? url.href : "";
  } catch {
    return "";
  }
}

function takeLoginToken() {
  const url = new URL(window.location.href);
  const fragment = new URLSearchParams(url.hash.replace(/^#/u, ""));
  const token = fragment.get("login_token");
  if (!token) return null;
  fragment.delete("login_token");
  url.hash = fragment.toString() ? `#${fragment.toString()}` : "";
  window.history.replaceState({}, "", `${url.pathname}${url.search}${url.hash}`);
  return token;
}

function handleLoginTokenFragment() {
  const token = takeLoginToken();
  if (!token) return;
  void finishLogin(token).catch((error) => showToast(error.message, "error"));
}
