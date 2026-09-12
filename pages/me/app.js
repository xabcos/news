import { $, api, node, button, withBusy, toast, reportError, openDialog, closeDialog, bindDialogs, confirmDelete, safeUrl, formatTime, cost } from "./ui.js";
import { createAccount } from "./account.js";

let session = null;
let authController;
const form = $("#topicForm");
const numericFields = ["window_start", "window_end", "max_items", "lookback_hours", "daily_run_cap", "sort_weight"];
const publicFields = ["name_en", "description", "model", "reasoning_effort"];
const statusText = { ok: "完成", failed: "失败", running: "运行中", interrupted: "已中断" };

bindDialogs();
bindEvents();
buildScheduleInputs();
void authenticate(takeLoginToken());

function beginAuthRequest() {
  authController?.abort();
  authController = new AbortController();
  return authController.signal;
}

async function authenticate(token) {
  const signal = beginAuthRequest();
  endSession();
  $("#authView").classList.add("hidden");
  $("#appView").classList.add("hidden");
  try {
    if (token) await api("/api/auth/email/consume", { method: "POST", body: { token }, signal });
    await enterApp(await api("/api/me", { signal }));
  } catch (error) {
    if (!signal.aborted) {
      showAuth();
      if (token || error.status !== 401) reportError(error);
    }
  }
}

function bindEvents() {
  $("#loginForm").addEventListener("submit", (event) => {
    event.preventDefault();
    void withBusy($("#loginForm button"), "正在发送…", async () => {
      const data = await api("/api/auth/email/start", { method: "POST", body: { email: $("#email").value.trim() }, signal: beginAuthRequest() });
      $("#sentEmail").textContent = data.email;
      $("#loginForm").classList.add("hidden");
      $("#mailSent").classList.remove("hidden");
      const url = safeUrl(data.login_url, true);
      const link = $("#devLoginLink");
      link.classList.toggle("hidden", !url);
      if (url) link.href = url;
      else link.removeAttribute("href");
      (url ? link : $("#sendAgain")).focus();
    });
  });
  $("#sendAgain").addEventListener("click", resetLoginForm);
  window.addEventListener("hashchange", () => {
    const token = takeLoginToken();
    if (token) void authenticate(token);
  });
  $("#logoutButton").addEventListener("click", () => withBusy($("#logoutButton"), "退出中…", async () => {
    await session.request("/api/auth/logout", { method: "POST" });
    showAuth();
  }));
  $("#newTopicButton").addEventListener("click", () => openEditor());
  $("#refreshButton").addEventListener("click", () => withBusy($("#refreshButton"), null, () => refresh(session)));
  $("#refreshItemsButton").addEventListener("click", () => withBusy($("#refreshItemsButton"), null, () => loadItems(session)));
  $("#itemsDay").addEventListener("change", () => void loadItems(session));
  $("#latestButton").addEventListener("click", () => { $("#itemsDay").value = ""; void loadItems(session); });
  $("#runsPanel").addEventListener("toggle", () => {
    if ($("#runsPanel").open && session?.admin) void loadRuns(session);
  });
  form.noValidate = true;
  form.addEventListener("submit", saveTopic);
  $("#deleteTopicButton").addEventListener("click", deleteTopic);
  $("#copyUrlButton").addEventListener("click", () => withBusy($("#copyUrlButton"), "复制中…", async () => {
    await navigator.clipboard.writeText(`${location.origin}/digest?topic=${encodeURIComponent(session.editing.id)}`);
    toast("公共地址已复制");
  }));
  document.addEventListener("visibilitychange", () => {
    const s = session;
    if (!s) return;
    clearTimeout(s.timer);
    if (document.visibilityState === "visible" && !s.refreshing) void refresh(s).catch(reportError);
  });
}

function buildScheduleInputs() {
  for (const [id, start] of [["windowStart", 0], ["windowEnd", 1]]) {
    $(`#${id}`).replaceChildren(...Array.from({ length: 24 }, (_, i) => {
      const option = node("option", "", `${i + start}:00`);
      option.value = String(i + start);
      return option;
    }));
  }
  $("#weekdays").replaceChildren(...["一", "二", "三", "四", "五", "六", "日"].map((day, i) => {
    const label = node("label");
    const input = node("input");
    input.type = "checkbox";
    input.name = "weekdays";
    input.value = String(i + 1);
    label.append(input, `周${day}`);
    return label;
  }));
}

function takeLoginToken() {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const token = fragment.get("login_token");
  if (token) {
    fragment.delete("login_token");
    history.replaceState(null, "", `${location.pathname}${location.search}${fragment.size ? `#${fragment}` : ""}`);
  }
  return token;
}

function resetLoginForm() {
  $("#loginForm").classList.remove("hidden");
  $("#mailSent").classList.add("hidden");
  $("#sentEmail").textContent = "";
  $("#devLoginLink").removeAttribute("href");
  $("#email").focus();
}

function endSession() {
  if (session) {
    session.controller.abort();
    clearTimeout(session.timer);
    session.account?.destroy();
  }
  session = null;
  document.querySelectorAll("dialog[open]").forEach(closeDialog);
  form.reset();
  for (const id of ["topicList", "itemList", "runsBody", "accountSummary", "planOffer", "ordersList", "paymentContent"]) $(`#${id}`).replaceChildren();
  for (const id of ["contentTitle", "itemsMeta", "runsMessage", "topicFormMessage", "confirmTitle", "confirmMessage"]) $(`#${id}`).textContent = "";
  $("#contentPanel").classList.add("hidden");
  $("#runsPanel").open = false;
  $("#itemsDay").value = "";
  $("#accountEmail").textContent = "";
  $("#statusLine").textContent = "";
  $("#confirmActionButton").onclick = null;
}

function showAuth() {
  endSession();
  $("#appView").classList.add("hidden");
  $("#authView").classList.remove("hidden");
  resetLoginForm();
}

async function enterApp(data) {
  const s = {
    user: data.user, usage: data.usage, admin: data.user.role === "admin",
    controller: new AbortController(), topics: [], running: new Set(), selected: null,
    itemsVersion: 0, runsVersion: 0, refreshVersion: 0, timer: null, refreshing: false,
    timezone: data.usage.timezone, today: { runs: data.usage.runs_today, cost_ticks: 0 },
    users: null,
  };
  session = s;
  s.base = s.admin ? "/api/admin" : "/api/me";
  s.request = async (path, options) => {
    try { return await api(path, { ...options, signal: s.controller.signal }); }
    catch (error) {
      if (error.status === 401 || (error.status === 403 && error.message === "当前账户不可用")) {
        if (session === s) { showAuth(); toast(error.message, "error"); }
        throw new DOMException("Session ended", "AbortError");
      }
      throw error;
    }
  };
  if (!s.admin) s.account = createAccount(s, () => refresh(s));
  $("#authView").classList.add("hidden");
  $("#appView").classList.remove("hidden");
  $("#accountEmail").textContent = s.user.email;
  $("#workspaceTitle").textContent = s.admin ? "公共主题" : "我的主题";
  $("#accountButton").classList.toggle("hidden", s.admin);
  $("#dateTools").classList.toggle("hidden", !s.admin);
  $("#runsPanel").classList.toggle("hidden", !s.admin);
  $("#newTopicButton").disabled = true;
  message("topicsMessage", "");
  try { await refresh(s); }
  catch (error) {
    if (session === s) { message("topicsMessage", "主题载入失败，请点击刷新重试。"); reportError(error); }
  }
}

async function refresh(s) {
  if (!s || session !== s) return;
  if (s.refreshing) return;
  s.refreshing = true;
  clearTimeout(s.timer);
  const version = ++s.refreshVersion;
  try {
    const [topics, status, account] = await Promise.all([
      s.request(`${s.base}/topics`), s.request(`${s.base}/status`),
      s.admin ? null : s.request("/api/me"),
    ]);
    if (session !== s || version !== s.refreshVersion) return;
    const finished = [...s.running].some((id) => !status.running.includes(id));
    const selected = topics.find((topic) => topic.id === s.selected);
    const contentChanged = selected?.last_run_at !== s.topics.find((topic) => topic.id === s.selected)?.last_run_at;
    s.topics = topics;
    s.running = new Set(status.running);
    s.timezone = status.timezone;
    s.today = status.today;
    s.users = status.users || null;
    if (account) { s.user = account.user; s.usage = account.usage; s.account.update(); }
    if (!selected) {
      await selectTopic(s, topics[0]?.id);
    } else {
      renderTopics(s);
      $("#contentTitle").textContent = selected.name;
      if (finished || contentChanged) await loadItems(s);
    }
    if (s.admin && $("#runsPanel").open) await loadRuns(s);
  } finally {
    s.refreshing = false;
    if (session === s && version === s.refreshVersion) scheduleRefresh(s);
  }
}

function scheduleRefresh(s) {
  clearTimeout(s.timer);
  if (document.visibilityState !== "visible") return;
  s.timer = setTimeout(async () => {
    if (session !== s) return;
    if (document.visibilityState !== "visible") return;
    try { await refresh(s); } catch { /* The next scheduled refresh retries. */ }
  }, s.running.size ? 5000 : 60000);
}

function message(id, text) {
  const element = $(`#${id}`);
  element.textContent = text;
  element.classList.toggle("hidden", !text);
}

function renderTopics(s) {
  $("#statusLine").textContent = s.admin
    ? `${s.users?.active_7d ?? 0}/${s.users?.registered ?? 0} · 今日 ${s.today.runs} 次 · ${cost(s.today.cost_ticks)}`
    : `今日运行 ${s.today.runs} / ${s.user.limits.daily_run_cap}`;
  $("#newTopicButton").disabled = !s.admin && s.topics.length >= s.user.limits.max_topics;
  message("topicsMessage", s.topics.length ? "" : "还没有主题，创建一个你关心的方向。");
  $("#topicList").replaceChildren(...s.topics.map((topic) => {
    const row = node("li", "topic-row");
    const select = node("button", "topic-select");
    select.type = "button";
    if (s.selected === topic.id) select.setAttribute("aria-current", "true");
    const title = node("strong");
    const running = s.running.has(topic.id);
    const status = topic.plan_locked || !topic.enabled ? "" : running ? "running" : topic.last_status || "ok";
    const dot = node("span", `dot ${status}`);
    dot.setAttribute("aria-hidden", "true");
    title.append(dot, topic.name);
    const interval = s.admin ? topic.interval_minutes : Math.max(topic.interval_minutes, s.user.limits.min_interval_minutes);
    const detail = topic.plan_locked ? "当前套餐不包含 · 升级 Pro 后恢复" : !topic.enabled ? "已暂停" : running ? "正在抓取…"
      : `${interval} 分钟${topic.next_run_at ? ` · 下次 ${formatTime(topic.next_run_at, s.timezone)}` : " · 等待排程"}${topic.last_status === "failed" ? " · 上次失败" : ""}`;
    select.append(title, node("span", "topic-meta", detail));
    select.addEventListener("click", () => void selectTopic(s, topic.id));
    const actions = node("div", "topic-actions");
    const run = button(running ? "抓取中" : "抓取", () => runTopic(s, topic), "button-secondary");
    run.disabled = running || !topic.enabled || topic.plan_locked === true;
    if (topic.plan_locked) run.title = "升级 Pro 后恢复";
    else if (!topic.enabled) run.title = "请先启用主题";
    actions.append(run, button("编辑", () => openEditor(topic)));
    row.append(select, actions);
    return row;
  }));
}

async function selectTopic(s, id) {
  if (!s || session !== s) return;
  s.selected = s.topics.find((topic) => topic.id === id)?.id || s.topics[0]?.id || null;
  s.itemsVersion += 1;
  s.runsVersion += 1;
  $("#runsBody").replaceChildren();
  $("#runsPanel").open = false;
  $("#itemsDay").value = "";
  $("#contentPanel").classList.toggle("hidden", !s.selected);
  $("#contentTitle").textContent = "";
  $("#itemsMeta").textContent = "";
  $("#itemList").replaceChildren();
  renderTopics(s);
  if (s.selected) await loadItems(s);
}

async function loadItems(s) {
  if (!s || session !== s || !s.selected) return;
  const id = s.selected;
  const topic = s.topics.find((topic) => topic.id === id);
  if (!topic) return;
  const version = ++s.itemsVersion;
  const day = s.admin ? $("#itemsDay").value : "";
  $("#contentTitle").textContent = topic.name;
  $("#itemsMeta").textContent = "";
  $("#itemList").replaceChildren();
  message("itemsMessage", "");
  try {
    const items = await s.request(`${s.base}/topics/${encodeURIComponent(id)}/items${day ? `?day=${day}` : ""}`);
    if (session !== s || version !== s.itemsVersion) return;
    $("#itemsMeta").textContent = `${day || items[0]?.day || "最近内容"} · ${items.length} 条`;
    message("itemsMessage", items.length ? "" : "暂时没有内容，主题运行后会显示在这里。");
    $("#itemList").replaceChildren(...items.map((item) => renderItem(s, item)));
  } catch (error) {
    if (session === s && version === s.itemsVersion) message("itemsMessage", error.message);
  }
}

function renderItem(s, item) {
  const row = node("li", `item-row${item.pinned ? " is-pinned" : ""}${item.hidden ? " is-hidden" : ""}`);
  const url = safeUrl(item.url);
  const link = node(url ? "a" : "strong", "", item.title);
  if (url) { link.href = url; link.target = "_blank"; link.rel = "noopener noreferrer"; }
  const meta = node("div", "item-meta");
  meta.append(node("span", "", item.source), node("span", "", `${formatTime(item.first_seen_at, s.timezone)} 收录`));
  for (const tag of item.tags) meta.append(node("span", "", tag));
  if (s.admin) meta.append(node("span", "", `重要度 ${item.importance} · 出现 ${item.seen_count} 次`));
  row.append(link, node("p", "", item.summary), meta);
  if (s.admin) {
    const actions = node("div", "item-actions");
    const patch = async (body) => {
      await s.request(`${s.base}/items/${item.id}`, { method: "PATCH", body });
      if (s.selected === item.topic_id) await loadItems(s);
    };
    actions.append(button(item.pinned ? "取消置顶" : "置顶", () => patch({ pinned: !item.pinned })),
      button(item.hidden ? "显示" : "隐藏", () => patch({ hidden: !item.hidden })),
      button("删除", () => confirmDelete("删除这条内容？", "删除后不会再出现在公开页面。", async () => {
        await s.request(`${s.base}/items/${item.id}`, { method: "DELETE" });
        if (s.selected === item.topic_id) await loadItems(s);
      }), "button-quiet danger-text"));
    row.append(actions);
  }
  return row;
}

async function loadRuns(s) {
  if (!s || session !== s || !s.admin || !s.selected) return;
  const version = ++s.runsVersion;
  message("runsMessage", "");
  try {
    const runs = await s.request(`${s.base}/topics/${encodeURIComponent(s.selected)}/runs?limit=50`);
    if (session !== s || version !== s.runsVersion) return;
    message("runsMessage", runs.length ? `最近 ${runs.length} 次` : "还没有运行记录");
    $("#runsBody").replaceChildren(...runs.map((run) => {
      const row = node("tr");
      const values = [
        `${formatTime(run.started_at, s.timezone)}${run.triggered_by === "manual" ? " 手动" : ""}`,
        statusText[run.status] || run.status,
        `${run.returned_count} / ${run.new_count} / ${run.dup_count} / ${run.dropped_count}`,
        `${run.input_tokens.toLocaleString()} / ${run.output_tokens.toLocaleString()}`,
        `${run.web_search_calls} / ${run.x_search_calls}`, cost(run.cost_ticks),
        run.finished_at ? `${Math.round((Date.parse(run.finished_at) - Date.parse(run.started_at)) / 1000)}s` : "…", run.error || "",
      ];
      row.append(...values.map((value, i) => node("td", i === 7 ? "error-cell" : "", value)));
      return row;
    }));
  } catch (error) {
    if (session === s && version === s.runsVersion) message("runsMessage", error.message);
  }
}

function openEditor(topic = null) {
  const s = session;
  if (!s || (!topic && !s.admin && s.topics.length >= s.user.limits.max_topics)) return;
  s.editing = topic;
  form.reset();
  message("topicFormMessage", "");
  $("#topicDialogTitle").textContent = topic ? "编辑主题" : "新建主题";
  $("#advancedSettings").classList.toggle("hidden", !s.admin);
  $("#advancedSettings").open = false;
  $("#adminFields").disabled = !s.admin;
  $("#deleteTopicButton").classList.toggle("hidden", !topic);
  $("#copyUrlButton").classList.toggle("hidden", !s.admin || !topic);
  const minimum = s.admin ? 15 : s.user.limits.min_interval_minutes;
  const values = {
    name: "", brief: "", id: "", name_en: "", description: "", model: "", reasoning_effort: "low",
    window_start: 6, window_end: 24, max_items: 8, lookback_hours: 24, daily_run_cap: 24, sort_weight: 0,
    ...topic,
    interval_minutes: Math.max(minimum, topic?.interval_minutes || (s.admin ? 60 : minimum)),
  };
  for (const key of ["name", "brief", "id", "interval_minutes", ...publicFields, ...numericFields]) form.elements.namedItem(key).value = values[key];
  $("#topicId").readOnly = Boolean(topic);
  $("#topicEnabled").checked = topic?.enabled ?? true;
  $("#topicInterval").min = String(minimum);
  $("#intervalHint").textContent = s.admin ? "最短 15 分钟" : `${s.user.plan === "pro" ? "Pro" : "Free"} 最短 ${minimum} 分钟 · 每天 06:00–24:00`;
  $("#scheduleTimezone").textContent = `排程时区：${s.timezone}`;
  $("#weekdays").querySelectorAll("input").forEach((input) => { input.checked = (topic?.weekdays || [1, 2, 3, 4, 5]).includes(Number(input.value)); });
  openDialog($("#topicDialog"));
  $("#topicName").focus();
}

function saveTopic(event) {
  event.preventDefault();
  const s = session;
  if (!form.checkValidity()) {
    if ($("#adminFields :invalid")) $("#advancedSettings").open = true;
    form.reportValidity();
    return;
  }
  const data = new FormData(form);
  const body = { name: data.get("name").trim(), brief: data.get("brief").trim(), interval_minutes: Number(data.get("interval_minutes")), enabled: $("#topicEnabled").checked };
  if (!body.name || !body.brief) return message("topicFormMessage", "请填写名称和关注方向");
  if (s.admin) {
    for (const key of publicFields) body[key] = data.get(key).trim();
    for (const key of numericFields) body[key] = Number(data.get(key));
    body.weekdays = data.getAll("weekdays").map(Number);
    if (body.window_start >= body.window_end || !body.weekdays.length) {
      $("#advancedSettings").open = true;
      return message("topicFormMessage", "结束时间须晚于开始时间，并至少选择一天");
    }
    if (!s.editing && data.get("id").trim()) body.id = data.get("id").trim();
  }
  const topic = s.editing;
  void withBusy($("#saveTopicButton"), "保存中…", async () => {
    message("topicFormMessage", "");
    let saved;
    try {
      saved = await s.request(`${s.base}/topics${topic ? `/${encodeURIComponent(topic.id)}` : ""}`, { method: topic ? "PATCH" : "POST", body });
    } catch (error) {
      if (session === s) message("topicFormMessage", error.message);
      return;
    }
    closeDialog($("#topicDialog"));
    toast(topic ? "主题已保存" : "主题已创建");
    await refresh(s);
    await selectTopic(s, saved.id);
  });
}

function deleteTopic() {
  const s = session;
  const topic = s.editing;
  if (!topic || $("#topicForm [aria-busy='true']")) return;
  confirmDelete("删除这个主题？", `“${topic.name}”及其全部内容和运行记录都会永久删除。`, async () => {
    await s.request(`${s.base}/topics/${encodeURIComponent(topic.id)}`, { method: "DELETE" });
    s.topics = s.topics.filter((entry) => entry.id !== topic.id);
    s.running.delete(topic.id);
    s.usage.topic_count = s.topics.length;
    s.account?.update();
    if (s.selected === topic.id) void selectTopic(s, s.topics[0]?.id);
    else renderTopics(s);
    closeDialog($("#topicDialog"));
    toast("主题已删除");
    void refresh(s).catch(reportError);
  });
}

async function runTopic(s, topic) {
  if (session !== s || s.running.has(topic.id)) return;
  s.running.add(topic.id);
  renderTopics(s);
  try {
    await s.request(`${s.base}/topics/${encodeURIComponent(topic.id)}/run`, { method: "POST" });
  } catch (error) {
    if (session === s) { s.running.delete(topic.id); renderTopics(s); }
    throw error;
  }
  toast("抓取已开始，完成后会自动刷新");
  await refresh(s);
}
