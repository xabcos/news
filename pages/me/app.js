import { $, api, node, button, withBusy, toast, reportError, openDialog, closeDialog, bindDialogs, confirmDelete, safeUrl, formatTime, cost } from "./ui.js";
import { createAccount } from "./account.js";

let session = null;
let authController;
let loginToken = null;
const form = $("#topicForm");
const numericFields = ["window_end", "max_items", "lookback_hours", "daily_run_cap", "sort_weight"];
const publicFields = ["name_en", "description", "model", "reasoning_effort"];
const statusText = { ok: "完成", failed: "失败", running: "运行中", interrupted: "已中断" };
// 𝑳𝑳𝑴 keeps a model catalogue instead of news items; its panel shows providers and changes.
const CATALOG_ID = "llm";
const DAILY_INTERVAL = 1440;
const pad = (value) => String(value ?? 0).padStart(2, "0");
const startTime = (topic) => `${pad(topic.window_start)}:${pad(topic.window_start_minute)}`;
const sourceKinds = { pricing: "定价页", models: "模型页", changelog: "更新日志" };
const changeKinds = { added: "新增", price: "调价", spec: "规格", status: "状态", removed: "下线", vendor_added: "服务商上线", vendor_removed: "服务商下线" };
const vendorStates = { active: "在线", candidate: "候选", offline: "已下线" };

bindDialogs();
bindEvents();
buildScheduleInputs();
const linkToken = takeLoginToken();
void (linkToken ? confirmLogin(linkToken) : authenticate());

function beginAuthRequest() {
  authController?.abort();
  authController = new AbortController();
  return authController.signal;
}

// A link signs in only after its address is confirmed, so a link someone else requested
// cannot silently switch this browser into their account.
async function confirmLogin(token) {
  const signal = beginAuthRequest();
  try {
    const { email } = await api("/api/auth/email/check", { method: "POST", body: { token }, signal });
    showAuth();
    loginToken = token;
    $("#confirmEmail").textContent = email;
    $("#loginForm").classList.add("hidden");
    $("#loginConfirm").classList.remove("hidden");
    $("#confirmLogin").focus();
  } catch (error) {
    if (!signal.aborted) {
      showAuth();
      reportError(error);
    }
  }
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
  $("#rejectLogin").addEventListener("click", resetLoginForm);
  $("#confirmLogin").addEventListener("click", () => void authenticate(loginToken));
  window.addEventListener("hashchange", () => {
    const token = takeLoginToken();
    if (token) void confirmLogin(token);
  });
  $("#logoutButton").addEventListener("click", () => withBusy($("#logoutButton"), "退出中…", async () => {
    await session.request("/api/auth/logout", { method: "POST" });
    showAuth();
  }));
  $("#newTopicButton").addEventListener("click", () => openEditor());
  $("#refreshButton").addEventListener("click", () => withBusy($("#refreshButton"), null, () => refresh(session)));
  $("#runBriefingButton").addEventListener("click", () => {
    const control = $("#runBriefingButton");
    void withBusy(control, "生成中…", () => runBriefing(session)).then(() => {
      if (session?.briefing?.status === "running") control.disabled = true;
    });
  });
  $("#copyWechatButton").addEventListener("click", () => copyBriefingDraft("wechatDraft", "copyWechatButton"));
  $("#copyXButton").addEventListener("click", () => copyBriefingDraft("xDraft", "copyXButton"));
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
    if (document.visibilityState === "visible") void refresh(s).catch(reportError);
  });
}

function buildScheduleInputs() {
  $("#windowEnd").replaceChildren(...Array.from({ length: 24 }, (_, i) => {
    const option = node("option", "", `${i + 1}:00`);
    option.value = String(i + 1);
    return option;
  }));
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
  loginToken = null;
  $("#loginForm").classList.remove("hidden");
  $("#mailSent").classList.add("hidden");
  $("#loginConfirm").classList.add("hidden");
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
  $("#briefingPanel").classList.add("hidden");
  $("#equityPanel").classList.add("hidden");
  $("#equityMeta").textContent = "";
  message("equityMessage", "");
  $("#briefingImages").classList.add("hidden");
  $("#briefingImages").replaceChildren();
  $("#briefingDrafts").classList.add("hidden");
  $("#briefingMessage").textContent = "";
  $("#briefingMeta").textContent = "";
  $("#wechatDraft").value = "";
  $("#xDraft").value = "";
  $("#runBriefingButton").disabled = false;
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
    itemsVersion: 0, runsVersion: 0, timer: null, refreshing: false,
    briefing: null,
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
  $("#briefingPanel").classList.toggle("hidden", !s.admin);
  $("#equityPanel").classList.toggle("hidden", !s.admin);
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
  try {
    const [topics, status, account] = await Promise.all([
      s.request(`${s.base}/topics`), s.request(`${s.base}/status`),
      s.admin ? null : s.request("/api/me"),
    ]);
    if (session !== s) return;
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
    if (s.admin) await Promise.all([loadBriefing(s), loadEquity(s)]);
  } finally {
    s.refreshing = false;
    if (session === s) scheduleRefresh(s);
  }
}

function scheduleRefresh(s) {
  clearTimeout(s.timer);
  if (document.visibilityState !== "visible") return;
  s.timer = setTimeout(async () => {
    if (session !== s) return;
    if (document.visibilityState !== "visible") return;
    try { await refresh(s); } catch { /* The next scheduled refresh retries. */ }
  }, 3 * 60 * 1000);
}

async function loadBriefing(s) {
  if (!s || session !== s || !s.admin) return;
  try {
    const payload = await s.request("/api/admin/insight");
    if (session !== s) return;
    s.briefing = payload.briefing;
    renderBriefing(s);
  } catch (error) {
    if (session === s) message("briefingMessage", error.message);
  }
}

async function loadEquity(s) {
  try {
    const { briefing, enabled } = await s.request("/api/admin/equity-insight");
    if (session !== s) return;
    const status = { running: "采集与生成中", ok: "已完成", failed: "失败" };
    $("#equityMeta").textContent = briefing
      ? `${briefing.briefing_day} · ${briefing.edition === "evening" ? "晚间" : "早间"} · ${status[briefing.status]} · ${formatTime(briefing.finished_at || briefing.started_at, s.timezone)}`
      : "每天早晚两期，研判近 7 天股市资料，保留历史笔记";
    $("#runEquityButton").disabled = !enabled || briefing?.status === "running";
    message("equityMessage", !enabled ? "启用 equity 主题抓取后可生成研判；前台展示可保持关闭。" : briefing?.error || "");
  } catch (error) {
    if (session === s) message("equityMessage", error.message);
  }
}

$("#runEquityButton").addEventListener("click", () => {
  const s = session;
  if (!s?.admin) return;
  void withBusy($("#runEquityButton"), "提交中…", async () => {
    await s.request("/api/admin/equity-insight", { method: "POST" });
    toast("股市研判已开始生成");
  }).then(() => { if (session === s) return loadEquity(s); }).catch(reportError);
});

function renderBriefing(s) {
  const briefing = s.briefing;
  if (!briefing) {
    message("briefingMessage", "还没有生成过 XAI 日报。");
    $("#briefingMeta").textContent = "每日 06:00 开始采集，完成后生成日报；也可重新生成最近一期";
    $("#briefingDrafts").classList.add("hidden");
    $("#runBriefingButton").disabled = false;
    renderBriefingImages([]);
    return;
  }
  const labels = { running: "采集与生成中", ok: "已完成", failed: "失败", interrupted: "已中断" };
  const day = briefing.briefing_day || "";
  const time = briefing.finished_at || briefing.started_at;
  const hasDraft = Boolean(briefing.wechat_article || briefing.x_article);
  const fallbackLabel = hasDraft && briefing.status !== "ok" ? " · 显示上一份可用稿" : "";
  $("#briefingMeta").textContent = `${day}${time ? ` · ${formatTime(time, s.timezone)}` : ""} · ${labels[briefing.status] || briefing.status}${fallbackLabel}`;
  $("#runBriefingButton").disabled = briefing.status === "running";
  message("briefingMessage", briefing.error || "");
  $("#briefingDrafts").classList.toggle("hidden", !hasDraft);
  $("#wechatDraft").value = briefing.wechat_article || "";
  $("#xDraft").value = briefing.x_article || "";
  renderBriefingImages(briefing.images || []);
}

function renderBriefingImages(images) {
  const container = $("#briefingImages");
  container.replaceChildren();
  container.classList.toggle("is-two", images.length === 2);
  container.classList.toggle("hidden", !images.length);
  if (!images.length) return;
  container.append(...images.map((image) => {
    const card = node("figure", "briefing-image");
    const url = safeUrl(image.url);
    if (url) {
      const img = node("img");
      img.src = url;
      img.alt = image.caption || "日报配图";
      img.loading = "lazy";
      img.referrerPolicy = "no-referrer";
      card.append(img);
    }
    const label = image.cover ? "封面" : "正文配图";
    const caption = node("figcaption", "briefing-image-caption", `${label}${image.archived ? " · 资料图" : ""} · ${image.caption || "日报配图"}`);
    const origin = safeUrl(image.source_url);
    if (origin) {
      const source = node("a", "briefing-image-source", `${image.credit || "图片来源"} ↗`);
      source.href = origin;
      source.target = "_blank";
      source.rel = "noopener noreferrer";
      caption.append(source);
    }
    card.append(caption);
    return card;
  }));
}

async function runBriefing(s) {
  if (!s || session !== s || !s.admin) return;
  await s.request("/api/admin/insight", { method: "POST" });
  toast("XAI 日报已开始生成，完成后会自动刷新");
  await loadBriefing(s);
}

function copyBriefingDraft(id, buttonId) {
  const value = $(`#${id}`).value;
  if (!value) return;
  void withBusy($(`#${buttonId}`), "复制中…", async () => {
    const text = value.replace(/\r\n?/g, "\n").replace(/\n[ \t]*\n(?:[ \t]*\n)+/g, "\n\n").trim();
    const html = id === "xDraft" ? session?.briefing?.x_html : "";
    if (html && typeof ClipboardItem !== "undefined" && navigator.clipboard.write) {
      try {
        await navigator.clipboard.write([new ClipboardItem({
          "text/plain": new Blob([text], { type: "text/plain" }),
          "text/html": new Blob([html], { type: "text/html" }),
        })]);
        toast("已复制排版稿，可直接粘贴到 X Articles");
        return;
      } catch { /* Some browsers only allow plain-text clipboard writes. */ }
    }
    await navigator.clipboard.writeText(text);
    toast(html ? "已复制纯文本稿" : "稿件已复制");
  });
}

function message(id, text) {
  const element = $(`#${id}`);
  element.textContent = text;
  element.classList.toggle("hidden", !text);
}

function canCreatePersonalTopic(s) {
  return s.admin || s.user.plan === "pro";
}

function updateEditorPlanState(s) {
  const topic = s.editing;
  const canCreate = canCreatePersonalTopic(s);
  const minimum = s.admin ? 15 : s.user.limits.min_interval_minutes;
  $("#topicInterval").min = String(minimum);
  $("#intervalHint").textContent = s.admin
    ? "最短 15 分钟 · 1440 表示每天在开始时间运行一次 · 结束时间早于开始时间表示跨午夜"
    : !canCreate && !topic
      ? `可先配置任务，购买 Pro 后创建 · 最短 ${minimum} 分钟`
      : `${s.user.plan === "pro" ? "Pro" : "Free"} 最短 ${minimum} 分钟 · 结束时间早于开始时间表示跨午夜`;
  $("#saveTopicButton").textContent = !topic && !s.admin && !canCreate ? "购买 Pro 后创建" : "保存主题";
}

function renderTopics(s) {
  $("#statusLine").textContent = s.admin
    ? `${s.users?.active_7d ?? 0}/${s.users?.registered ?? 0} · 今日 ${s.today.runs} 次 · ${cost(s.today.cost_ticks)}`
    : `今日运行 ${s.today.runs} / ${s.user.limits.daily_run_cap}`;
  const canCreate = canCreatePersonalTopic(s);
  const newTopicButton = $("#newTopicButton");
  newTopicButton.disabled = !s.admin && canCreate && s.topics.length >= s.user.limits.max_topics;
  newTopicButton.textContent = !s.admin && !canCreate ? "＋ 新建主题（Pro）" : "＋ 新建主题";
  newTopicButton.title = !s.admin && !canCreate ? "购买 Pro 后创建抓取任务" : "";
  updateEditorPlanState(s);
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
    const cadence = interval >= DAILY_INTERVAL ? `每天 ${startTime(topic)}` : `${interval} 分钟`;
    const detail = topic.plan_locked ? "当前套餐不包含 · 升级 Pro 后恢复" : !topic.enabled ? "已暂停" : running ? "正在抓取…"
      : `${cadence}${topic.next_run_at ? ` · 下次 ${formatTime(topic.next_run_at, s.timezone)}` : " · 等待排程"}${topic.last_status === "failed" ? " · 上次失败" : ""}`;
    select.append(title, node("span", "topic-meta", `${detail}${topic.visible ? "" : " · 前台隐藏"}`));
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
  message("itemsMessage", "");
  const catalog = s.admin && id === CATALOG_ID;
  $("#dateTools").classList.toggle("hidden", !s.admin || catalog);
  if (catalog) return loadCatalog(s, version);
  // Switching topics clears the list first; a reload of the same list replaces it in place.
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
  const url = item.topic_id === "reads" && !item.hidden ? `/reads/${item.id}` : safeUrl(item.url);
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
  if (!s) return;
  const canCreate = canCreatePersonalTopic(s);
  if (!topic && !s.admin && canCreate && s.topics.length >= s.user.limits.max_topics) return;
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
    window_start: 6, window_start_minute: 0, window_end: 24, max_items: 8, lookback_hours: 24, daily_run_cap: 24, sort_weight: 0,
    ...topic,
    interval_minutes: Math.max(minimum, topic?.interval_minutes || (s.admin ? 60 : minimum)),
  };
  for (const key of ["name", "brief", "id", "interval_minutes", ...publicFields, ...numericFields]) form.elements.namedItem(key).value = values[key];
  $("#windowStart").value = startTime(values);
  document.querySelectorAll("[data-news-only]").forEach((field) => { field.hidden = topic?.id === CATALOG_ID; });
  $("#topicId").readOnly = Boolean(topic);
  $("#topicEnabled").checked = topic?.enabled ?? true;
  $("#topicVisible").checked = topic?.visible ?? true;
  $("#scheduleTimezone").textContent = `排程时区：${s.timezone} · 结束早于开始表示运行到次日`;
  $("#weekdays").querySelectorAll("input").forEach((input) => { input.checked = (topic?.weekdays || [1, 2, 3, 4, 5]).includes(Number(input.value)); });
  updateEditorPlanState(s);
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
  const body = { name: data.get("name").trim(), brief: data.get("brief").trim(), interval_minutes: Number(data.get("interval_minutes")), enabled: $("#topicEnabled").checked, visible: $("#topicVisible").checked };
  if (!body.name || !body.brief) return message("topicFormMessage", "请填写名称和关注方向");
  if (s.admin) {
    for (const key of publicFields) body[key] = data.get(key).trim();
    for (const key of numericFields) body[key] = Number(data.get(key));
    const [hour, minute] = String(data.get("window_start_time")).split(":").map(Number);
    body.window_start = hour;
    body.window_start_minute = minute || 0;
    body.weekdays = data.getAll("weekdays").map(Number);
    if (body.window_start === body.window_end || !body.weekdays.length) {
      $("#advancedSettings").open = true;
      return message("topicFormMessage", "开始和结束时间不能相同；结束早于开始表示跨午夜，并至少选择一天");
    }
    if (!s.editing && data.get("id").trim()) body.id = data.get("id").trim();
  }
  const topic = s.editing;
  if (!topic && !s.admin && !canCreatePersonalTopic(s)) {
    void s.account.promptUpgrade();
    return;
  }
  void withBusy($("#saveTopicButton"), "保存中…", async () => {
    message("topicFormMessage", "");
    let saved;
    try {
      saved = await s.request(`${s.base}/topics${topic ? `/${encodeURIComponent(topic.id)}` : ""}`, { method: topic ? "PATCH" : "POST", body });
    } catch (error) {
      if (!topic && !s.admin && error.status === 403) {
        void s.account.promptUpgrade();
        return;
      }
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

async function loadCatalog(s, version) {
  try {
    const data = await s.request("/api/admin/llm");
    if (session !== s || version !== s.itemsVersion) return;
    const shown = data.models.filter((model) => model.shown).length;
    const online = data.vendors.filter((vendor) => vendor.enabled && vendor.state === "active").length;
    const waiting = data.vendors.filter((vendor) => vendor.state === "candidate").length;
    $("#itemsMeta").textContent = `${online} 家在线${waiting ? ` · ${waiting} 家候选` : ""} · ${shown} 个模型展示中`;
    const rows = data.vendors.map((vendor) => renderVendor(s, vendor, data.models.filter((model) => model.vendor_id === vendor.id)));
    if (data.changes.length) rows.push(renderChanges(s, data.changes));
    $("#itemList").replaceChildren(...rows);
  } catch (error) {
    if (session === s && version === s.itemsVersion) message("itemsMessage", error.message);
  }
}

function catalogLink(text, url) {
  const href = safeUrl(url);
  const link = node(href ? "a" : "span", "", text);
  if (href) { link.href = href; link.target = "_blank"; link.rel = "noopener noreferrer"; }
  return link;
}

// Values come only from the providers' pages and providers join and leave on their own;
// the workspace can pause a provider or hide a model.
function renderVendor(s, vendor, models) {
  const listed = vendor.enabled && vendor.state === "active";
  const row = node("li", `item-row catalog-vendor${listed ? "" : " is-hidden"}`);
  const meta = node("div", "item-meta");
  meta.append(node("span", "", vendor.region === "cn" ? "国内" : "海外"),
    node("span", "", vendor.enabled ? vendorStates[vendor.state] || vendor.state : "已暂停"));
  if (vendor.origin === "discovered") meta.append(node("span", "", "自动发现"));
  if (vendor.state === "active") meta.append(node("span", "", `${vendor.models} 个模型`));
  meta.append(node("span", "", vendor.verified_at ? `核验于 ${formatTime(vendor.verified_at, s.timezone)}`
    : vendor.checked_at ? `尝试于 ${formatTime(vendor.checked_at, s.timezone)}` : "尚未读取"));
  if (vendor.held_since) meta.append(node("span", "", `暂缓下线自 ${formatTime(vendor.held_since, s.timezone)}`));
  const title = node("strong");
  title.append(catalogLink(vendor.name, vendor.homepage));
  row.append(title, meta);
  if (vendor.error) row.append(node("p", "catalog-error", vendor.error));
  const sources = node("ul", "catalog-sources");
  for (const source of vendor.sources) {
    const item = node("li");
    const state = source.error ? `失败 ${source.failures} 次：${source.error}`
      : source.checked_at ? `检查于 ${formatTime(source.checked_at, s.timezone)}${source.changed_at ? ` · 内容更新于 ${formatTime(source.changed_at, s.timezone)}` : ""}` : "尚未检查";
    item.append(catalogLink(`${sourceKinds[source.kind] || source.kind}${source.currency ? ` · ${source.currency}` : ""}`, source.url),
      node("span", source.error ? "catalog-error" : "muted", state));
    sources.append(item);
  }
  if (!vendor.sources.length) sources.append(node("li", "muted", "尚未登记官方页面"));
  row.append(sources);
  if (models.length) {
    const details = node("details", "catalog-models");
    details.append(node("summary", "", `模型（${models.length}）`));
    const list = node("ul");
    for (const model of models) {
      const item = node("li", model.shown ? "" : "is-hidden");
      const state = model.removed_at ? " · 已下线" : model.hidden ? " · 已隐藏" : model.shown ? "" : " · 未展示（旧版本或无价格）";
      const label = node("span", "", `${model.model_id} · ${model.name}${state}`);
      item.append(label);
      if (!model.removed_at) {
        item.append(button(model.hidden ? "显示" : "隐藏", async () => {
          await s.request(`/api/admin/llm/models/${encodeURIComponent(model.vendor_id)}/${encodeURIComponent(model.model_id)}`, { method: "PATCH", body: { hidden: !model.hidden } });
          await loadItems(s);
        }));
      }
      list.append(item);
    }
    details.append(list);
    row.append(details);
  }
  const actions = node("div", "item-actions");
  actions.append(button(vendor.enabled ? "暂停该服务商" : "启用该服务商", async () => {
    await s.request(`/api/admin/llm/vendors/${encodeURIComponent(vendor.id)}`, { method: "PATCH", body: { enabled: !vendor.enabled } });
    await loadItems(s);
  }));
  row.append(actions);
  return row;
}

function renderChanges(s, changes) {
  const row = node("li", "item-row");
  const list = node("ul", "catalog-sources");
  for (const change of changes) {
    const subject = change.model_id ? `${change.vendor_id} · ${change.model_id}` : change.vendor_id;
    list.append(node("li", "", `${formatTime(change.detected_at, s.timezone)} · ${changeKinds[change.kind] || change.kind} · ${subject}`));
  }
  row.append(catalogLink("最近变化 ↗", "/llm"), list);
  return row;
}
