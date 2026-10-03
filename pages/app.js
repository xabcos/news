"use strict";

const $ = (selector) => document.querySelector(selector);
const REFRESH_MS = 5 * 60 * 1000;
// Items per topic per request. The first screen asks for one page; scrolling to the end of
// a group silently fetches that group's next page. ALL starts every topic with fewer items,
// which halves its first response.
const PAGE_LIMIT = 12;
const ALL_PAGE_LIMIT = 6;
// Delay before a failed page request is retried, still without any visible message.
const PAGE_RETRY_MS = 8000;
const FETCH_TIMEOUT_MS = 20_000;
// Browsers with scroll anchoring keep the viewport still when cards are inserted above it;
// elsewhere the scroll position is corrected by hand.
const NATIVE_SCROLL_ANCHORING = typeof CSS !== "undefined" && CSS.supports?.("overflow-anchor", "auto") === true;
// Set at login beside the HttpOnly session cookie. It only chooses which feed the page asks
// for first; the server authorizes that request as usual.
const ROLE_COOKIE = "news_role";
// Keep the simple timeline bounded to the archive window exposed by the API.
const DAYS_LIMIT = 366;
const PRELOAD_MARGIN = "0px 0px 70% 0px";
const INSIGHT_ID = "insight";
const EQUITY_ID = "equity-insight";
// The photograph gallery: rows of photographs at their own ratio, opened in a lightbox.
const PHOTO_TOPIC = "w";
// Travelers' postcards in packed columns, with a map of recent destinations (travel.js).
const TRAVEL_TOPIC = "travel";
const storage = (() => {
  try {
    const store = window.localStorage;
    return {
      getItem(key) {
        try {
          return store.getItem(key);
        } catch {
          return null;
        }
      },
      setItem(key, value) {
        try {
          store.setItem(key, value);
        } catch {
          // Private browsing and full storage should not break the public page.
        }
      },
    };
  } catch {
    return { getItem: () => null, setItem: () => {} };
  }
})();

const TEXT = {
  zh: {
    locale: "zh-CN", lang: "EN", all: "𝐀𝐋𝐋", insight: "𝐀𝐈", today: "今天", top: "重要",
    fresh: "NEW", failed: "加载失败", retry: "重试",
    empty: "这一天还没有内容", latest: "查看最近有内容的一天",
    space: "工作台", personalEmpty: "还没有可阅读的内容", manageTopics: "管理我的主题",
    backToTop: "回到顶部", insightEmpty: "XAI 日报将在今日生成后显示。", insightLabel: "XAI 日报",
    hotspots: "今日热点", gossip: "AI 圈热议", references: "参考来源", archive: "资料图", changes: "趋势分析", dataSignals: "关键数据", watchSignals: "后续观察", coverSource: "原文", wealth: "商业与机遇", employment: "就业与技能",
    risks: "风险预警", actions: "观察方向",
    kind: "类型", horizon: "时间", beneficiaries: "受益者", entry: "切入方式", watch: "观察信号", risk: "主要风险",
    confidence: (n, grade) => `证据强度 ${n}/5${grade ? ` · 证据 ${grade}` : ""}`, sources: "来源",
    items: (n) => `${n} 条`, photos: (n) => `${n} 张`,
    close: "关闭", previous: "上一张", next: "下一张", play: "播放", pause: "暂停", review: "评测", shared: "分享",
  },
  en: {
    locale: "en", lang: "中文", all: "𝐀𝐋𝐋", insight: "𝐀𝐈", today: "Today", top: "Top",
    fresh: "NEW", failed: "Failed to load", retry: "Retry",
    empty: "Nothing collected for this day yet", latest: "Go to the latest day with content",
    space: "𝑾𝒐𝒓𝒌𝒔𝒑𝒂𝒄𝒆", personalEmpty: "Nothing to read yet", manageTopics: "Manage my topics",
    backToTop: "Back to top", insightEmpty: "XAI Daily will appear after today's edition is generated.", insightLabel: "XAI Daily",
    hotspots: "Today’s headlines", gossip: "AI buzz", references: "References", archive: "Archive photo", changes: "Trend analysis", dataSignals: "Key data", watchSignals: "Signals ahead", coverSource: "Source", wealth: "Business & opportunities", employment: "Employment & skills",
    risks: "Risk alerts", actions: "What to watch",
    kind: "Type", horizon: "Horizon", beneficiaries: "Who may benefit", entry: "Entry path", watch: "Signals to watch", risk: "Main risk",
    confidence: (n, grade) => `Evidence ${n}/5${grade ? ` · Grade ${grade}` : ""}`, sources: "Sources",
    items: (n) => `${n} ${n === 1 ? "item" : "items"}`, photos: (n) => `${n} ${n === 1 ? "photo" : "photos"}`,
    close: "Close", previous: "Previous", next: "Next", play: "Play", pause: "Pause", review: "Review", shared: "Shared",
  },
};

const initial = new URLSearchParams(location.search);
const state = {
  lang: initial.has("lang") ? (initial.get("lang") === "en" ? "en" : "zh") : (storage.getItem("news.lang") === "en" ? "en" : "zh"),
  topic: initial.get("topic") || "",
  day: initial.get("day") || "",
  at: initial.get("topic") === EQUITY_ID ? initial.get("at") || "" : "",
  topics: [],
  days: [],
  // One digest per loaded day; navigation/reload starts a fresh, bounded timeline.
  timeline: [],
  insight: null,
  insightEtag: null,
  personal: false,
  historyLoading: false,
  historyError: false,
  historyDone: false,
  // Items first collected after the previous visit get a NEW mark.
  lastVisit: Number(storage.getItem("news.lastVisit")) || Date.now(),
  epoch: 0,
};
storage.setItem("news.lastVisit", String(Date.now()));

const el = {
  tabs: $("#tabs"), lang: $("#langToggle"), me: $("#meLink"),
  shell: $(".shell"), status: $("#status"), digest: $("#digest"), loadMore: $("#loadMore"),
  backToTop: $("#backToTop"),
};
const t = () => TEXT[state.lang];
// Sentinels at the end of groups with more items on the server, and the group each belongs to.
const groupTargets = new WeakMap();
const groupObserver = new IntersectionObserver(
  (entries) => entries.forEach((entry) => entry.isIntersecting && loadMoreItems(entry.target)),
  { rootMargin: PRELOAD_MARGIN },
);
let activeLoadController = null;
let travelMap = null;

document.addEventListener("DOMContentLoaded", init);

// Catalogue sections share one tab over their topics, which have server-rendered pages.
const CATALOG = {
  culture: { label: "𝑪𝒖𝒍𝒕𝒖𝒓𝒆", topics: ["movie", "music", "game", "book"] },
  gadgets: { label: "𝑮𝒂𝒅𝒈𝒆𝒕𝒔", topics: ["eink", "ai-hardware", "digital"] },
};
const sectionOf = (id) => Object.keys(CATALOG).find((section) => CATALOG[section].topics.includes(id));
// Topics with their own server-rendered page instead of a news feed.
const PAGE_TOPICS = ["reads", "llm", ...Object.keys(CATALOG), ...Object.values(CATALOG).flatMap((section) => section.topics)];
const topicPage = (id) => PublicPage.readPath(`/${sectionOf(id) ? `${sectionOf(id)}/${id}` : id}${state.lang === "en" ? "?lang=en" : ""}`);

async function init() {
  if (PAGE_TOPICS.includes(state.topic)) {
    location.replace(topicPage(state.topic));
    return;
  }
  applyLang();
  el.lang.addEventListener("click", () => {
    const scrollTop = window.scrollY;
    state.lang = state.lang === "zh" ? "en" : "zh";
    storage.setItem("news.lang", state.lang);
    applyLang();
    renderTabs();
    if (state.topic === INSIGHT_ID) renderInsight(state.insight, true, scrollTop);
    else if (state.topic === EQUITY_ID) renderEquity(true, scrollTop);
    else render(true, scrollTop);
  });
  window.addEventListener("popstate", () => {
    const params = new URLSearchParams(location.search);
    state.topic = params.get("topic") || "";
    state.day = state.topic === INSIGHT_ID ? "" : params.get("day") || "";
    state.at = state.topic === EQUITY_ID ? params.get("at") || "" : "";
    renderTabs();
    load();
  });
  // A refresh redraws today's cards, so it waits while a photograph viewer walks them and
  // while the reader is below the first screen, where the redraw would move the page.
  setInterval(() => {
    if (document.visibilityState === "visible" && !state.day && !state.at
      && window.scrollY < window.innerHeight
      && !document.body.classList.contains("dialog-open")) load(true);
  }, REFRESH_MS);
  PublicPage.init({ canSwipe: (target) => Boolean(target.closest("#digest"))
    || (!el.digest.childElementCount && !target.closest("a")) });
  setupInfiniteScroll();

  // Everything the first screen needs leaves in one round trip: the role cookie replaces the
  // old `/api/me` probe, and the catalogue loads beside the digest instead of before it.
  state.personal = viewerRole() === "user";
  renderTabs();
  await Promise.all([loadCatalog(), load()]);
}

async function fetchJson(path, { signal, withEtag = false } = {}) {
  const privatePath = path.startsWith("/api/");
  const controller = new AbortController();
  let timedOut = false;
  const timer = window.setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, FETCH_TIMEOUT_MS);
  const abort = () => controller.abort();
  if (signal) {
    if (signal.aborted) controller.abort();
    else signal.addEventListener("abort", abort, { once: true });
  }
  try {
    const response = await fetch(path, {
      credentials: "same-origin",
      cache: privatePath ? "no-store" : "default",
      headers: { Accept: "application/json" },
      signal: controller.signal,
    });
    if (!response.ok) {
      const error = new Error(`HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    const data = await response.json();
    return withEtag ? { data, etag: response.headers.get("ETag") } : data;
  } catch (error) {
    if (timedOut) {
      const timeout = new Error("Request timed out");
      timeout.name = "TimeoutError";
      throw timeout;
    }
    throw error;
  } finally {
    window.clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
  }
}

function startLoad() {
  activeLoadController?.abort();
  activeLoadController = new AbortController();
  return activeLoadController;
}

// The role cookie is the only hint about the session that the public page reads. Anonymous
// readers therefore never touch a private endpoint, and every request they make is cacheable.
const viewerRole = PublicPage.role;

// The server clears the cookie on logout; this covers a session that expired on its own.
function clearViewerRole() {
  const secure = location.protocol === "https:" ? "; Secure" : "";
  document.cookie = `${ROLE_COOKIE}=; Path=/; Max-Age=0; SameSite=Lax${secure}`;
}

async function loadCatalog() {
  if (state.personal) return;
  try {
    state.topics = (await fetchJson(PublicPage.readPath("/topics"))).topics;
    renderTabs();
  } catch {
    // The digest still loads; the tabs just stay at ALL and AI.
  }
}

// Logged-in tabs come from /api/me/feed, which the insight path does not otherwise call.
async function personalCatalog(signal) {
  try {
    const payload = await fetchJson(`/api/me/feed?limit=1`, { signal });
    return { topics: payload.topics || [], expired: false };
  } catch (error) {
    if (error.name === "AbortError") throw error;
    if ([401, 403].includes(error.status)) return { topics: null, expired: true };
    return { topics: null, expired: false };
  }
}

function search(extra = {}) {
  const params = new URLSearchParams();
  if (state.topic) params.set("topic", state.topic);
  if (state.day) params.set("day", state.day);
  if (state.topic === EQUITY_ID && state.at) params.set("at", state.at);
  Object.entries(extra).forEach(([key, value]) => params.set(key, value));
  const query = params.toString();
  return query ? `?${query}` : "";
}

function firstPageLimit() {
  return state.topic ? PAGE_LIMIT : ALL_PAGE_LIMIT;
}

function catalogPath() {
  const params = new URLSearchParams();
  if (state.topic) params.set("topic", state.topic);
  params.set("limit", String(DAYS_LIMIT));
  return PublicPage.readPath(`/days?${params}`);
}

function dayPath(day) {
  const params = new URLSearchParams({ day, limit: String(firstPageLimit()) });
  if (state.topic) params.set("topic", state.topic);
  return `${state.personal ? "/api/me/feed" : PublicPage.readPath("/digest")}?${params}`;
}

function pagePath(day, topic, after) {
  const params = new URLSearchParams({ topic, day, limit: String(PAGE_LIMIT), after });
  return `${state.personal ? "/api/me/feed" : PublicPage.readPath("/digest")}?${params}`;
}

// A background refresh trusts the server's feed revision. An unchanged group keeps its
// already loaded pages; a changed group starts a fresh page sequence and sheds stale cards.
function refreshGroups(fresh, current) {
  const known = new Map((current.topics || []).map((group) => [group.id, group]));
  const topics = (fresh.topics || []).map((group) => {
    const previous = known.get(group.id);
    if (!previous || previous.revision !== group.revision) return group;
    return { ...group, items: previous.items, next: previous.next };
  });
  return { ...fresh, topics };
}

function digestGroupsChanged(next, previous) {
  const groups = next || [];
  const oldGroups = previous || [];
  if (groups.length !== oldGroups.length) return true;
  return groups.some((group, index) => {
    const old = oldGroups[index];
    return group.id !== old.id
      || group.revision !== old.revision
      || group.total !== old.total
      || group.next !== old.next;
  });
}

// Adds one page to a group and moves its cursor on. Ids are checked because an item
// unpinned between two requests comes back once more further down.
function appendPage(group, page) {
  const seen = new Set(group.items.map((item) => item.id));
  const added = page.items.filter((item) => !seen.has(item.id));
  group.items.push(...added);
  group.next = page.next || null;
  group.total = page.total;
  return added;
}

function clearDigest() {
  groupObserver.disconnect();
  el.digest.replaceChildren();
  el.shell.classList.add("is-loading");
}

async function load(silent = false) {
  if ([INSIGHT_ID, EQUITY_ID].includes(state.topic)) return loadInsight(silent);
  const controller = startLoad();
  const { signal } = controller;
  const epoch = ++state.epoch;
  if (!silent) {
    state.timeline = [];
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = false;
    clearDigest();
    updateHistoryLoader();
    setStatus("");
  }
  try {
    let digest;
    let days;
    if (state.personal) {
      const payload = await fetchJson(`/api/me/feed${search({ limit: firstPageLimit() })}`, { signal });
      digest = payload.digest;
      days = payload.days;
      if (epoch === state.epoch) state.topics = payload.topics;
    } else {
      const [data, archive] = await Promise.all([
        fetchJson(PublicPage.readPath(`/digest${search({ limit: firstPageLimit() })}`), { signal }),
        fetchJson(catalogPath(), { signal }),
      ]);
      digest = data;
      days = archive.days;
    }
    if (epoch !== state.epoch) return;
    const current = state.timeline[0];
    if (silent && current && digest.day === current.day) {
      digest = refreshGroups(digest, current);
    }
    state.days = days.map((entry) => entry.day);
    if (state.personal) renderTabs();
    // A background refresh only repaints when something actually changed.
    if (silent
      && digest.day === current?.day
      && !digestGroupsChanged(digest.topics, current?.topics)) {
      updateHistoryBoundary();
      setStatus("");
      updateHistoryLoader();
      return;
    }
    // Preserve loaded history only while the latest day stays the same or advances.
    if (silent && current?.day && digest.day >= current.day) {
      if (current.day === digest.day) state.timeline[0] = digest;
      else state.timeline.unshift(digest);
    } else {
      state.timeline = [digest];
      state.historyLoading = false;
    }
    state.historyError = false;
    updateHistoryBoundary();
    setStatus("");
    render(silent, window.scrollY, { latestOnly: silent && current?.day === digest.day });
  } catch (error) {
    if (error.name === "AbortError") return;
    // If a session expires while the page is open, switch back to the public feed
    // instead of leaving a private page permanently stuck on an auth error.
    if ((state.personal || viewerRole() === "admin") && [401, 403].includes(error.status) && epoch === state.epoch) {
      clearViewerRole();
      state.personal = false;
      state.topic = "";
      state.day = "";
      state.topics = [];
      state.days = [];
      history.replaceState({}, "", `${location.pathname}${search()}`);
      renderTabs();
      await Promise.all([loadCatalog(), load()]);
      renderTabs();
      return;
    }
    // A link can still contain a topic outside this account's visible catalogue (for
    // example, another user's private topic). Drop that selector instead of leaving the
    // page on a confusing 404.
    if (state.personal && error.status === 404 && state.topic && epoch === state.epoch) {
      state.topic = "";
      state.day = "";
      history.replaceState({}, "", `${location.pathname}${search()}`);
      renderTabs();
      await load();
      return;
    }
    if (epoch === state.epoch && !silent) setStatus(t().failed, true);
  }
}

async function loadInsight(silent = false) {
  const equity = state.topic === EQUITY_ID;
  const key = equity ? "equity" : "insight";
  const controller = startLoad();
  const { signal } = controller;
  const epoch = ++state.epoch;
  if (!equity && state.day) {
    state.day = "";
    history.replaceState({}, "", `${location.pathname}${search()}`);
  }
  if (!silent) {
    state.timeline = [];
    state.days = [];
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = true;
    clearDigest();
    updateHistoryLoader();
    setStatus("");
  }
  try {
    const needCatalog = state.personal && !state.topics.length;
    const selector = state.at ? `at=${encodeURIComponent(state.at)}` : state.day ? `day=${encodeURIComponent(state.day)}` : "";
    const [{ data: payload, etag }, catalog] = await Promise.all([
      fetchJson(equity ? `/equity-insight${selector ? `?${selector}` : ""}` : "/insight", { signal, withEtag: true }),
      needCatalog ? personalCatalog(signal) : null,
    ]);
    if (epoch !== state.epoch) return;
    if (catalog?.expired) {
      clearViewerRole();
      state.personal = false;
      await loadCatalog();
    } else if (catalog?.topics) {
      state.topics = catalog.topics;
      renderTabs();
    }
    if (silent && etag && etag === state[`${key}Etag`]) {
      state.historyLoading = false;
      state.historyError = false;
      state.historyDone = true;
      setStatus("");
      return;
    }
    state[`${key}Etag`] = etag;
    state[key] = equity ? payload : payload.insight;
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = true;
    setStatus("");
    if (equity) renderEquity(silent);
    else renderInsight(state.insight, silent);
  } catch (error) {
    if (error.name === "AbortError") return;
    if (epoch === state.epoch && !silent) setStatus(t().failed, true);
  }
}

function nextTimelineDay() {
  const lastDay = state.timeline[state.timeline.length - 1]?.day;
  if (!lastDay) return state.days[0];
  const index = state.days.indexOf(lastDay);
  if (index >= 0) return state.days[index + 1];
  return state.days.find((day) => day < lastDay);
}

function updateHistoryBoundary() {
  state.historyDone = !nextTimelineDay();
}

async function loadOlder() {
  if (state.historyLoading || state.historyError || state.historyDone || !state.timeline.length) return;
  // The day on screen is finished before the previous one appears: a group the reader
  // scrolled past while it still had pages is completed first, in reading order.
  const pending = el.digest.querySelector(".day-block:last-of-type .group-more");
  if (pending) {
    loadMoreItems(pending);
    return;
  }
  const day = nextTimelineDay();
  if (!day) {
    state.historyDone = true;
    updateHistoryLoader();
    return;
  }
  // A five-minute refresh may replace today's digest while this request is in flight.
  // Keep the request attached to this timeline, rather than the main-load epoch, so a
  // harmless refresh cannot leave the loader stuck in its "loading" state.
  const timeline = state.timeline;
  const personal = state.personal;
  state.historyLoading = true;
  state.historyError = false;
  updateHistoryLoader();
  try {
    const payload = await fetchJson(dayPath(day), { signal: activeLoadController?.signal });
    if (timeline !== state.timeline) return;
    const digest = personal ? payload.digest : payload;
    if (!digest?.day || state.timeline.some((entry) => entry.day === digest.day)) {
      state.historyDone = true;
      return;
    }
    state.timeline.push(digest);
    el.digest.append(renderDayBlock(digest, true));
    updateHistoryBoundary();
  } catch (error) {
    if (error.name === "AbortError") return;
    if (timeline === state.timeline) state.historyError = true;
  } finally {
    if (timeline === state.timeline) {
      state.historyLoading = false;
      updateHistoryLoader();
      window.setTimeout(maybeLoadOlder, 0);
    }
  }
}

function maybeLoadOlder() {
  if (state.historyLoading || state.historyError || state.historyDone || !state.timeline.length) return;
  const remaining = document.documentElement.scrollHeight - window.scrollY - window.innerHeight;
  if (remaining < window.innerHeight * .7) loadOlder();
}

function setupInfiniteScroll() {
  new IntersectionObserver(
    (entries) => entries.some((entry) => entry.isIntersecting) && loadOlder(),
    { rootMargin: PRELOAD_MARGIN },
  ).observe(el.loadMore);
}

// An empty element after the last card of a group that has more items on the server.
// Scrolling it into view fetches the next page; there is no button and no label.
function groupSentinel(digest, group) {
  const sentinel = document.createElement("div");
  sentinel.className = "group-more";
  sentinel.setAttribute("aria-hidden", "true");
  groupTargets.set(sentinel, { digest, group, loading: false });
  groupObserver.observe(sentinel);
  return sentinel;
}

// Re-observing reports the current intersection again, so a sentinel that merely moved
// down (or one whose request failed) is re-checked without waiting for a scroll event.
function reobserve(sentinel) {
  groupObserver.unobserve(sentinel);
  groupObserver.observe(sentinel);
}

async function loadMoreItems(sentinel) {
  const target = groupTargets.get(sentinel);
  if (!target || target.loading || !sentinel.isConnected || !target.group.next) return;
  const { digest, group } = target;
  const timeline = state.timeline;
  target.loading = true;
  try {
    const payload = await fetchJson(pagePath(digest.day, group.id, group.next), { signal: activeLoadController?.signal });
    if (timeline !== state.timeline || !sentinel.isConnected) return;
    const page = (state.personal ? payload.digest : payload)?.topics?.find((entry) => entry.id === group.id);
    const added = page ? appendPage(group, page) : [];
    if (!page) group.next = null;
    insertBefore(sentinel, added.map((item) => renderGroupItem(group, item)));
    if (group.next) {
      target.loading = false;
      reobserve(sentinel);
    } else {
      groupObserver.unobserve(sentinel);
      sentinel.remove();
    }
    window.setTimeout(maybeLoadOlder, 0);
  } catch (error) {
    if (error.name === "AbortError") {
      target.loading = false;
      if (sentinel.isConnected) reobserve(sentinel);
      return;
    }
    window.setTimeout(() => {
      target.loading = false;
      if (sentinel.isConnected) reobserve(sentinel);
    }, PAGE_RETRY_MS);
  }
}

// Cards can land above the viewport when the bottom loader completes a group the reader
// already scrolled past; the page must not jump under them.
function insertBefore(sentinel, nodes) {
  if (!nodes.length) return;
  const before = sentinel.getBoundingClientRect().top;
  sentinel.before(...nodes);
  if (NATIVE_SCROLL_ANCHORING || before >= 0) return;
  window.scrollBy(0, sentinel.getBoundingClientRect().top - before);
}

function updateHistoryLoader() {
  el.loadMore.replaceChildren();
  if (state.historyError) {
    el.loadMore.append(t().failed, " ", button(t().retry, () => {
      state.historyError = false;
      loadOlder();
    }, "button button-quiet button-small"));
  }
}

function navigate(changes) {
  if (PAGE_TOPICS.includes(changes.topic)) {
    location.assign(topicPage(changes.topic));
    return;
  }
  Object.assign(state, { at: "" }, changes);
  history.pushState({}, "", `${location.pathname}${search()}`);
  renderTabs();
  load();
}

function setStatus(message, failed = false) {
  el.status.replaceChildren();
  if (!message) return;
  el.status.append(message);
  if (failed) {
    const retry = button(t().retry, () => load(), "button button-secondary button-small");
    el.status.append(" ", retry);
  }
}

function applyLang() {
  document.documentElement.lang = t().locale;
  el.lang.textContent = t().lang;
  el.me.textContent = t().space;
  el.backToTop.setAttribute("aria-label", t().backToTop);
  el.backToTop.title = t().backToTop;
  updateHistoryLoader();
}

function renderTabs() {
  // The visual feed gets a wider desktop canvas while every other topic keeps the
  // narrow reading measure. This class also updates when navigating between tabs.
  document.body.classList.toggle("p-topic", state.topic === "p");
  document.body.classList.toggle("w-topic", state.topic === PHOTO_TOPIC);
  document.body.classList.toggle("travel-topic", state.topic === TRAVEL_TOPIC);
  renderTravelMap();
  document.title = state.topic === INSIGHT_ID
    ? `𝑵𝒆𝒘𝒔 · ${state.lang === "en" ? "XAI Daily" : "XAI 日报"}`
    : state.topic === EQUITY_ID ? `𝑵𝒆𝒘𝒔 · ${state.lang === "en" ? "Equity notes" : "股市观察"}` : "𝑵𝒆𝒘𝒔";
  const shown = new Set();
  const catalogue = state.topics.flatMap((topic) => {
    const section = sectionOf(topic.id);
    if (!section) return [{ id: topic.id, label: name(topic) }];
    if (shown.has(section)) return [];
    shown.add(section);
    return [{ id: section, label: CATALOG[section].label }];
  });
  const tabs = [
    { id: "", label: t().all },
    { id: INSIGHT_ID, label: t().insight },
    ...catalogue,
  ];
  const overseas = tabs.findIndex((tab) => tab.id === "overseas-ai");
  tabs.splice(overseas < 0 ? 2 : overseas + 1, 0, { id: EQUITY_ID, label: "𝑬" });
  el.tabs.replaceChildren(...tabs.map(({ id, label }) => {
    if (PAGE_TOPICS.includes(id)) {
      const tab = document.createElement("a");
      tab.className = "tab";
      tab.href = topicPage(id);
      tab.textContent = label;
      // Swiping back into a catalogue section enters its last collection (public.js).
      if (CATALOG[id]) tab.dataset.last = topicPage(CATALOG[id].topics.findLast((topic) => state.topics.some((entry) => entry.id === topic)));
      return tab;
    }
    const tab = button(label, () => navigate({ topic: id, day: "" }), "tab");
    if (id === state.topic) tab.setAttribute("aria-current", "true");
    return tab;
  }));
  // Deep links, browser back/forward, and the async catalogue load can all select
  // a tab that is initially outside the strip's horizontal viewport.
  window.requestAnimationFrame(() => scrollActiveTab());
}

// The map sits above the travel tab's postcards and follows the page language.
function renderTravelMap() {
  if (state.topic !== TRAVEL_TOPIC) {
    travelMap?.remove();
    travelMap = null;
    return;
  }
  if (travelMap?.dataset.lang === state.lang) return;
  if (!travelMap) {
    travelMap = document.createElement("div");
    travelMap.className = "travel-map-slot";
    travelMap.hidden = true;
    el.digest.before(travelMap);
  }
  travelMap.dataset.lang = state.lang;
  TravelUI.map(travelMap);
}

function scrollActiveTab() {
  const active = el.tabs.querySelector('[aria-current="true"]');
  if (!active?.scrollIntoView) return;
  const tabsRect = el.tabs.getBoundingClientRect();
  const activeRect = active.getBoundingClientRect();
  if (activeRect.left >= tabsRect.left && activeRect.right <= tabsRect.right) return;
  active.scrollIntoView({
    behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
    block: "nearest",
    inline: "center",
  });
}

function render(preserveScroll = false, scrollTop = window.scrollY, { latestOnly = false } = {}) {
  if (!state.timeline.length) return;
  el.shell.classList.remove("is-loading");
  if (latestOnly && el.digest.firstElementChild) {
    // History keeps its observers while only the newest day's cards are replaced.
    el.digest.firstElementChild.querySelectorAll(".group-more").forEach((sentinel) => groupObserver.unobserve(sentinel));
    el.digest.firstElementChild.replaceWith(renderDayBlock(state.timeline[0], false));
    updateHistoryLoader();
    window.setTimeout(maybeLoadOlder, 0);
    if (preserveScroll) {
      window.requestAnimationFrame(() => { document.scrollingElement.scrollTop = scrollTop; });
    }
    return;
  }
  // Sentinels of the outgoing blocks must be dropped before the new blocks register theirs.
  groupObserver.disconnect();
  el.digest.replaceChildren(...state.timeline.map((digest, index) => {
    // Keep the latest block compact; dates remain visible on loaded history blocks.
    const showDayHeading = index > 0;
    return renderDayBlock(digest, showDayHeading);
  }));
  updateHistoryLoader();
  if (preserveScroll) {
    // Off-screen blocks settle after the first layout pass; restore after two paints.
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      document.scrollingElement.scrollTop = scrollTop;
    }));
  }
  window.setTimeout(maybeLoadOlder, 0);
}

function renderEquity(preserveScroll = false, scrollTop = window.scrollY) {
  clearDigest();
  el.digest.append(EquityNotes.render(state.equity, { lang: state.lang, onEdition: (at) => navigate({ topic: EQUITY_ID, day: "", at }) }));
  el.shell.classList.remove("is-loading");
  if (preserveScroll) window.requestAnimationFrame(() => { document.scrollingElement.scrollTop = scrollTop; });
}

function renderInsight(insight, preserveScroll = false, scrollTop = window.scrollY) {
  clearDigest();
  updateHistoryLoader();
  if (!insight) {
    const empty = document.createElement("div");
    empty.className = "empty muted";
    empty.textContent = t().insightEmpty;
    el.digest.append(empty);
    el.shell.classList.remove("is-loading");
    return;
  }
  const section = document.createElement("section");
  section.className = "insight";
  const heading = document.createElement("div");
  heading.className = "insight-heading";
  const briefingDate = `${t().insightLabel} · ${formatDay(insight.briefing_day, insight.timezone)}`;
  const title = document.createElement("h2");
  title.textContent = pick(insight, "headline");
  if (!insight.hotspots.length) {
    heading.append(span(briefingDate, "insight-date muted small"));
  }
  heading.append(title);
  section.append(heading);
  if (insight.images.length) section.append(renderInsightGallery(insight.images));
  const sources = new Map(insight.sources.map((source) => [source.index, source]));
  section.append(...renderInsightStories(t().hotspots, insight.hotspots, sources, { dateText: briefingDate }));
  section.append(...renderInsightStories(t().gossip, insight.gossip, sources, { gossip: true }));
  const trend = pick(insight, "changes");
  if (trend.trim()) {
    const changes = insightChange(trend, relativeTime(insight.generated_at));
    const changeSources = sourceList(insight.changes_source_indexes.map((index) => sources.get(index)).filter(Boolean));
    if (changeSources) changes.append(changeSources);
    section.append(changes);
  }
  section.append(...renderInsightGroup(t().dataSignals, insight.data_signals, sources));
  section.append(...renderInsightGroup(t().watchSignals, insight.watch_signals, sources));
  for (const [label, items, opportunities] of [
    [t().wealth, insight.wealth, true],
    [t().employment, insight.employment, false],
    [t().risks, insight.risks, false],
    [t().actions, insight.actions, false],
  ]) {
    section.append(...renderInsightGroup(label, items, sources, opportunities));
  }
  const references = insightSection(t().references);
  const referenceList = document.createElement("ol");
  referenceList.className = "insight-references";
  const seen = new Set();
  for (const source of [...sources.values(), ...insight.research_sources]) {
    if (seen.has(source.url)) continue;
    seen.add(source.url);
    const link = sourceLink(source.url, pick(source, "title") || source.url);
    if (!link) continue;
    const item = document.createElement("li");
    item.append(link);
    referenceList.append(item);
  }
  if (referenceList.children.length) {
    references.append(referenceList);
    section.append(references);
  }
  el.digest.append(section);
  el.shell.classList.remove("is-loading");
  if (preserveScroll) {
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      document.scrollingElement.scrollTop = scrollTop;
    }));
  }
}

function insightSection(label, dateText = "") {
  const section = document.createElement("section");
  section.className = "group insight-group";
  const heading = document.createElement("div");
  heading.className = "group-title";
  const title = document.createElement("h2");
  title.textContent = label;
  heading.append(title);
  if (dateText) heading.append(span(dateText, "insight-date insight-hotspots-date muted small"));
  section.append(heading);
  return section;
}

function renderInsightStories(label, stories, sources, { gossip = false, dateText = "" } = {}) {
  if (!stories.length) return [];
  const section = insightSection(label, dateText);
  const statuses = state.lang === "en"
    ? { confirmed: "Confirmed", reported: "Reported", unconfirmed: "Unconfirmed", response: "Response", feedback: "User feedback", denied: "Denied" }
    : { confirmed: "已确认", reported: "爆料", unconfirmed: "未证实", response: "当事方回应", feedback: "用户反馈", denied: "已否认" };
  section.append(...stories.map((story, index) => {
    const article = document.createElement("article");
    article.className = "item insight-hotspot";
    const title = document.createElement("h3");
    title.className = "item-title";
    const headline = document.createElement("span");
    if (gossip) headline.append(span(statuses[story.status] || statuses.unconfirmed, "insight-story-status"));
    headline.append(pick(story, "title"));
    title.append(span(String(index + 1).padStart(2, "0"), "insight-story-number"), headline);
    const summary = document.createElement("p");
    summary.className = "item-summary";
    summary.textContent = pick(story, "summary");
    article.append(title, summary);
    const links = sourceList((story.source_indexes || []).map((index) => sources.get(index)).filter(Boolean));
    if (links) article.append(links);
    return article;
  }));
  return [section];
}

function renderInsightGallery(images) {
  const gallery = document.createElement("div");
  gallery.className = `insight-gallery${images.length === 1 ? " is-single" : images.length === 2 ? " is-two" : ""}`;
  gallery.append(...images.map((image, index) => {
    const figure = document.createElement("figure");
    figure.className = "insight-gallery-item";
    const link = sourceLink(image.source_url || image.url, "");
    if (link) {
      link.className = "insight-gallery-media";
      const img = document.createElement("img");
      img.src = image.url;
      img.alt = pick(image, "caption");
      img.loading = index === 0 ? "eager" : "lazy";
      if (index === 0) img.fetchPriority = "high";
      img.decoding = "async";
      img.referrerPolicy = "no-referrer";
      img.addEventListener("error", () => figure.remove(), { once: true });
      link.append(img);
      figure.append(link);
    }
    const caption = document.createElement("figcaption");
    if (pick(image, "title")) caption.append(span(pick(image, "title"), "insight-picture-title"));
    const label = image.archived ? `${t().archive} · ` : "";
    caption.append(span(`${label}${pick(image, "caption")}`, "insight-caption"), span(` · ${image.credit} · `, "muted small"));
    const source = sourceLink(image.source_url, `${t().coverSource} ↗`);
    if (source) caption.append(source);
    figure.append(caption);
    return figure;
  }));
  return gallery;
}

function insightChange(text, meta) {
  const block = document.createElement("div");
  block.className = "insight-change";
  const heading = document.createElement("div");
  heading.className = "insight-change-heading";
  const title = document.createElement("h3");
  title.textContent = t().changes;
  heading.append(title);
  if (meta) heading.append(span(meta, "insight-change-meta muted small"));
  const body = document.createElement("p");
  body.append(insightText(text));
  block.append(heading, body);
  return block;
}

function renderInsightGroup(label, items, sources, opportunities = false) {
  if (!items.length) return [];
  const section = insightSection(label);
  section.append(...items.map((item) => insightCard(item, sources, opportunities)));
  return [section];
}

function insightCard(item, sources, opportunity = false) {
  const article = document.createElement("article");
  article.className = "item insight-item";
  const itemHeading = document.createElement("div");
  itemHeading.className = "insight-item-heading";
  const title = document.createElement("h3");
  title.className = "item-title";
  title.textContent = pick(item, "title");
  itemHeading.append(title, span(t().confidence(item.confidence, item.evidence_grade), "muted small"));
  const summary = document.createElement("p");
  summary.className = "item-summary";
  summary.append(insightText(pick(item, "summary")));
  article.append(itemHeading, summary);
  if (opportunity) article.append(opportunityDetails(item));
  const links = (item.source_indexes || [])
    .map((index) => sources.get(index))
    .filter(Boolean);
  const sourceLinks = sourceList(links);
  if (sourceLinks) article.append(sourceLinks);
  return article;
}

function opportunityDetails(item) {
  const fields = [
    [t().kind, opportunityKind(item.kind)],
    [t().horizon, opportunityHorizon(item.horizon)],
    [t().beneficiaries, pick(item, "beneficiaries")],
    [t().entry, pick(item, "entry")],
    [t().watch, pick(item, "watch")],
    [t().risk, pick(item, "risk")],
  ].filter(([, value]) => value);
  const details = document.createElement("div");
  details.className = "insight-opportunity-details";
  details.append(...fields.map(([label, value]) => {
    const row = document.createElement("div");
    row.className = "insight-opportunity-detail";
    row.append(span(label, "insight-opportunity-label"), span(value, "insight-opportunity-value"));
    return row;
  }));
  return details;
}

// Some Responses providers put citations in the JSON text as Markdown, including
// already stored briefings. Preserve those references without interpreting HTML.
function insightText(text) {
  const fragment = document.createDocumentFragment();
  const value = text || "";
  const citation = /\[([^\]\n]+)\]\((https?:\/\/(?:[^\s()]+|\([^\s()]*\))+)\)/gu;
  let offset = 0;
  for (const match of value.matchAll(citation)) {
    const link = sourceLink(match[2], match[1]);
    if (!link) continue;
    link.className = "insight-citation";
    fragment.append(value.slice(offset, match.index), link);
    offset = match.index + match[0].length;
  }
  fragment.append(value.slice(offset));
  return fragment;
}

function sourceList(sources) {
  const seen = new Set();
  const repeats = new Map();
  const anchors = sources.map((source) => {
    const link = sourceLink(source.url, "", pick(source, "title"));
    if (!link || seen.has(link.href)) return null;
    seen.add(link.href);
    // Several links from one outlet stay distinguishable: "Reuters ↗ Reuters 2 ↗".
    const name = source.source || hostOf(source.url);
    const count = (repeats.get(name) || 0) + 1;
    repeats.set(name, count);
    link.textContent = `${name}${count > 1 ? ` ${count}` : ""} ↗`;
    return link;
  }).filter(Boolean);
  if (!anchors.length) return null;
  const list = document.createElement("div");
  list.className = "insight-sources";
  list.append(span(t().sources, "muted small"), ...anchors);
  return list;
}

function opportunityKind(value) {
  const labels = {
    business: { zh: "商业机会", en: "Business" },
    income: { zh: "收入机会", en: "Income" },
    industry: { zh: "产业观察", en: "Industry" },
    asset_watch: { zh: "资产观察", en: "Asset watch" },
  };
  return labels[value]?.[state.lang] || value || "";
}

function opportunityHorizon(value) {
  const labels = {
    "0-3m": { zh: "0–3个月", en: "0–3 months" },
    "3-12m": { zh: "3–12个月", en: "3–12 months" },
    "12m+": { zh: "12个月以上", en: "12+ months" },
  };
  return labels[value]?.[state.lang] || value || "";
}

function renderDayBlock(digest, showDayHeading = true) {
  const block = document.createElement("section");
  block.className = "day-block";
  block.classList.toggle("day-heading-hidden", !showDayHeading);
  if (showDayHeading && digest.day) {
    const divider = document.createElement("div");
    divider.className = "day-divider";
    const title = document.createElement("h2");
    title.textContent = formatDay(digest.day, digest.timezone);
    divider.append(title);
    block.append(divider);
  }
  if (!totalItems(digest)) {
    block.append(emptyState(digest.topics || [], digest.day));
    return block;
  }
  block.append(...renderGroups(digest));
  return block;
}

function renderGroups(digest) {
  const groups = digest.topics || [];
  const visible = groups.filter((group) => (group.items || []).length);
  const single = visible.length === 1;
  return visible.map((group) => {
    const section = document.createElement("section");
    section.className = "group";
    section.classList.toggle("image-news-group", group.id === "p" || Boolean(sectionOf(group.id)));
    section.classList.toggle("photo-group", group.id === PHOTO_TOPIC);
    section.classList.toggle("catalog-book-group", group.id === "book");
    section.classList.toggle("postcard-group", group.id === TRAVEL_TOPIC);
    if (!single) {
      const heading = document.createElement("div");
      heading.className = "group-title";
      const title = document.createElement("h2");
      title.textContent = name(group);
      const count = document.createElement("p");
      count.className = "muted small";
      count.textContent = group.id === PHOTO_TOPIC ? t().photos(group.total) : t().items(group.total);
      heading.append(title, count);
      section.append(heading);
    }
    // Postcards pack in a grid of their own, below the group's heading.
    const cards = group.id === TRAVEL_TOPIC ? document.createElement("div") : section;
    if (cards !== section) {
      cards.className = "postcards";
      section.append(cards);
    }
    cards.append(...(group.items || []).map((item) => renderGroupItem(group, item)));
    if (group.next) cards.append(groupSentinel(digest, group));
    return section;
  });
}

function renderGroupItem(group, item) {
  if (group.id === PHOTO_TOPIC) return photoCard(item);
  if (group.id === TRAVEL_TOPIC) return TravelUI.postcard(item, { when: relativeTime(item.first_seen_at) });
  if (sectionOf(group.id)) {
    const article = imageNewsCard(item);
    article.classList.add("catalog-card", `catalog-${group.id}`);
    const role = { review: t().review, share: t().shared }[item.catalog?.role];
    if (role) article.querySelector(".item-meta").prepend(span(role, "badge catalog-role"));
    const body = document.createElement("div");
    body.className = "catalog-card-body";
    for (const child of [...article.children]) {
      if (!child.classList.contains("image-news-media")) body.append(child);
    }
    if (item.catalog?.path) {
      const link = document.createElement("a");
      link.className = "catalog-work-link";
      link.href = PublicPage.readPath(`${item.catalog.path}${state.lang === "en" ? "?lang=en" : ""}`);
      link.textContent = `${state.lang === "en" ? "Explore " : "了解 "}${pick(item.catalog, "name")} →`;
      body.append(link);
    }
    article.append(body);
    return article;
  }
  return group.id === "p" ? imageNewsCard(item) : card(item, group.id === "reads");
}

// Photographs of one row share a height; each keeps its own ratio, so none is cropped.
const photoItems = new WeakMap();
let lightbox = null;

function photoCard(item) {
  const figure = document.createElement("figure");
  figure.className = "photo";
  const { url = "", width, height } = item.image || {};
  figure.style.setProperty("--ratio", String(width > 0 && height > 0 ? width / height : 0.75));
  const link = document.createElement("a");
  link.className = "photo-media";
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  const image = document.createElement("img");
  image.src = url;
  image.alt = pick(item.image || {}, "caption") || pick(item, "title");
  image.loading = "lazy";
  image.decoding = "async";
  image.referrerPolicy = "no-referrer";
  // A photograph the file server no longer keeps hides its card; the lightbox steps over it.
  image.addEventListener("error", () => { figure.hidden = true; }, { once: true });
  link.append(image);
  // A plain click opens the lightbox; modified clicks still open the post.
  link.addEventListener("click", (event) => {
    if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    openLightbox(figure);
  });
  figure.append(link);
  if ([...(item.tags || []), ...(item.tags_en || [])].includes("AI")) figure.append(span("AI", "photo-badge"));
  const caption = document.createElement("figcaption");
  caption.className = "photo-caption";
  caption.append(span(pick(item, "title"), "photo-title"), span(photoCredit(item), "photo-credit"));
  figure.append(caption);
  photoItems.set(figure, item);
  return figure;
}

const photoCredit = (item) => item.image?.credit || item.source || hostOf(item.url);

function openLightbox(figure) {
  lightbox ||= createLightbox();
  lightbox.ui.open();
  showPhoto(figure);
}

function createLightbox() {
  const figure = document.createElement("figure");
  figure.className = "viewer-stage";
  const image = document.createElement("img");
  const caption = document.createElement("figcaption");
  caption.className = "viewer-caption";
  figure.append(image, caption);
  const ui = PublicPage.viewer({
    className: "is-solo",
    frame: figure,
    image,
    root: el.digest,
    step: stepPhoto,
    neighbor: neighborPhoto,
    ahead: () => photoItems.get(neighborPhoto(1))?.image?.url,
    names: () => ({ play: t().play, pause: t().pause, close: t().close, previous: t().previous, next: t().next }),
  });
  return { ui, image, caption, current: null };
}

// The photograph `offset` places from the open one. The lightbox remembers it by id: a
// background refresh draws new cards.
function neighborPhoto(offset) {
  const photos = [...el.digest.querySelectorAll(".photo")];
  let index = photos.findIndex((figure) => photoItems.get(figure)?.id === lightbox.current);
  if (index < 0) return null;
  do index += offset; while (photos[index]?.hidden);
  return photos[index] || null;
}

function stepPhoto(offset) {
  const target = neighborPhoto(offset);
  if (target) showPhoto(target);
  return target;
}

function showPhoto(figure) {
  const item = photoItems.get(figure);
  if (!item) return;
  const { image, caption, ui } = lightbox;
  lightbox.current = item.id;
  image.src = item.image?.url || "";
  image.alt = figure.querySelector("img")?.alt || "";
  caption.replaceChildren(span(pick(item, "title"), "viewer-caption-title"));
  const source = sourceLink(item.url, `${photoCredit(item)} ↗`);
  if (source) caption.append(source);
  ui.shown(figure);
}

function imageNewsCard(item) {
  const article = card(item);
  article.classList.add("image-news-card");
  const image = item.image?.url;
  if (!image) return article;
  const link = document.createElement("a");
  link.className = "image-news-media";
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  const element = document.createElement("img");
  element.src = image;
  element.alt = pick(item.image, "caption") || pick(item, "title");
  element.loading = "lazy";
  element.fetchPriority = "low";
  element.decoding = "async";
  element.referrerPolicy = "no-referrer";
  element.addEventListener("error", () => link.remove(), { once: true });
  link.append(element);
  article.prepend(link);
  // The headline follows the photograph; a caption that says more than the headline closes
  // the card with its credit.
  const caption = pick(item.image, "caption");
  if (caption && caption !== pick(item, "title")) {
    const credit = item.image.credit ? ` · ${item.image.credit}` : "";
    article.append(span(`${caption}${credit}`, "image-news-caption"));
  }
  return article;
}

function totalItems(digest) {
  return (digest?.topics || []).reduce((sum, group) => sum + (group.total || 0), 0);
}

function emptyState(groups, day) {
  const box = document.createElement("div");
  box.className = "empty muted";
  box.append(state.personal ? t().personalEmpty : t().empty);
  // A logged-in catalogue always contains public topics, so checking
  // `state.topics.length` here would hide the management link even when the
  // account has no personal topic (or the selected day is empty).
  if (state.personal) {
    box.append(document.createElement("br"));
    const manage = document.createElement("a");
    manage.className = "button button-secondary button-small";
    manage.href = "/me/";
    manage.textContent = t().manageTopics;
    box.append(manage);
  }
  const latest = groups.map((group) => group.latest_day).filter(Boolean).sort().pop();
  if (latest && latest !== day) {
    box.append(document.createElement("br"));
    box.append(button(`${t().latest} · ${latest}`, () => navigate({ day: "" }), "button button-secondary button-small"));
  }
  return box;
}

function card(item, reading = false) {
  const article = document.createElement("article");
  article.className = "item";
  article.classList.toggle("is-pinned", item.pinned);

  const meta = document.createElement("div");
  meta.className = "item-meta";
  // Publication dates are often only known to the day and arrive as midnight UTC;
  // the collection time is the reliable freshness signal shown in the feed.
  meta.append(span(item.source || hostOf(item.url), "item-source"), "·", span(relativeTime(item.first_seen_at), "item-time"));
  if (item.importance >= 4) meta.append(span(t().top, "badge badge-top"));
  if (Date.parse(item.first_seen_at) > state.lastVisit) meta.append(span(t().fresh, "badge badge-new"));

  const title = document.createElement("h3");
  title.className = "item-title";
  const link = document.createElement("a");
  link.href = reading ? PublicPage.readPath(`/reads/${item.id}${state.lang === "en" ? "?lang=en" : ""}`) : item.url;
  if (!reading) {
    link.target = "_blank";
    link.rel = "noopener noreferrer";
  }
  link.textContent = pick(item, "title");
  title.append(link);

  const summary = document.createElement("p");
  summary.className = "item-summary";
  summary.textContent = pick(item, "summary");

  article.append(meta, title, summary);
  const tags = state.lang === "en" && item.tags_en?.length ? item.tags_en : item.tags || [];
  if (tags.length) {
    const list = document.createElement("div");
    list.className = "item-tags";
    list.append(...tags.map((tag) => span(tag, "tag")));
    article.append(list);
  }
  return article;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const name = (topic) => (state.lang === "en" && topic.name_en) || topic.name;
const pick = (item, field) => (state.lang === "en" && item[`${field}_en`]) || item[field];
const sourceLink = PublicPage.link;
const formatterCache = new Map();

function formatter(kind, locale, options) {
  const key = `${kind}:${locale}:${JSON.stringify(options)}`;
  let value = formatterCache.get(key);
  if (!value) {
    value = kind === "relative"
      ? new Intl.RelativeTimeFormat(locale, options)
      : new Intl.DateTimeFormat(locale, options);
    formatterCache.set(key, value);
  }
  return value;
}

function button(label, onClick, className) {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = label;
  node.addEventListener("click", onClick);
  return node;
}

function span(text, className) {
  const node = document.createElement("span");
  node.className = className;
  node.textContent = text;
  return node;
}

function hostOf(url) {
  try {
    return new URL(url).hostname.replace(/^www\./u, "");
  } catch {
    return "";
  }
}

function todayIn(timeZone) {
  return formatter("date", "en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(new Date());
}

function formatDay(day, timeZone) {
  const [year, month, date] = day.split("-").map(Number);
  const label = formatter("date", t().locale, {
    year: year === new Date().getFullYear() ? undefined : "numeric",
    month: "long",
    day: "numeric",
    weekday: "long",
  }).format(new Date(year, month - 1, date));
  return day === todayIn(timeZone) ? `${t().today} · ${label}` : label;
}

function relativeTime(iso) {
  const timestamp = Date.parse(iso);
  if (!Number.isFinite(timestamp)) return "";
  const minutes = Math.round((timestamp - Date.now()) / 60000);
  const format = formatter("relative", t().locale, { numeric: "auto" });
  if (Math.abs(minutes) < 60) return format.format(minutes, "minute");
  if (Math.abs(minutes) < 60 * 24) return format.format(Math.round(minutes / 60), "hour");
  if (Math.abs(minutes) < 60 * 24 * 7) return format.format(Math.round(minutes / 1440), "day");
  return formatter("date", t().locale, { month: "short", day: "numeric" }).format(new Date(iso));
}
