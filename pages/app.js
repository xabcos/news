"use strict";

const $ = (selector) => document.querySelector(selector);
const REFRESH_MS = 5 * 60 * 1000;
// Items per topic per request. The first screen asks for one page; scrolling to the end of
// a group silently fetches that group's next page.
const PAGE_LIMIT = 12;
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
const SWIPE_MIN_DISTANCE = 48;
const SWIPE_AXIS_RATIO = 1.2;
const MOBILE_QUERY = "(max-width: 768px)";
const INSIGHT_ID = "insight";
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
    locale: "zh-CN", lang: "EN", catPortal: "旅行猫咪", catPortalAria: "进入旅行猫咪", all: "𝐀𝐋𝐋", insight: "𝐀𝐈", today: "今天", top: "重要",
    fresh: "NEW", failed: "加载失败", retry: "重试",
    empty: "这一天还没有内容", latest: "查看最近有内容的一天",
    space: "工作台", personalEmpty: "还没有可阅读的内容", manageTopics: "管理我的主题",
    backToTop: "回到顶部", insightEmpty: "𝐀𝐈 将在首次分析完成后显示。",
    changes: "趋势分析", coverSource: "原文", wealth: "商业与机遇", employment: "就业与技能",
    risks: "风险预警", actions: "观察方向", researchSources: "数据来源",
    kind: "类型", horizon: "时间", beneficiaries: "受益者", entry: "切入方式", watch: "观察信号", risk: "主要风险",
    confidence: (n) => `证据强度 ${n}/5`, sources: "来源",
    items: (n) => `${n} 条`,
  },
  en: {
    locale: "en", lang: "中文", catPortal: "𝑪𝒂𝒕", catPortalAria: "Open Cat", all: "𝐀𝐋𝐋", insight: "𝐀𝐈", today: "Today", top: "Top",
    fresh: "NEW", failed: "Failed to load", retry: "Retry",
    empty: "Nothing collected for this day yet", latest: "Go to the latest day with content",
    space: "𝑾𝒐𝒓𝒌𝒔𝒑𝒂𝒄𝒆", personalEmpty: "Nothing to read yet", manageTopics: "Manage my topics",
    backToTop: "Back to top", insightEmpty: "𝐀𝐈 will appear after its first analysis.",
    changes: "Trend analysis", coverSource: "Source", wealth: "Business & opportunities", employment: "Employment & skills",
    risks: "Risk alerts", actions: "What to watch", researchSources: "Data sources",
    kind: "Type", horizon: "Horizon", beneficiaries: "Who may benefit", entry: "Entry path", watch: "Signals to watch", risk: "Main risk",
    confidence: (n) => `Evidence ${n}/5`, sources: "Sources",
    items: (n) => `${n} ${n === 1 ? "item" : "items"}`,
  },
};

const initial = new URLSearchParams(location.search);
const state = {
  lang: storage.getItem("news.lang") === "en" ? "en" : "zh",
  topic: initial.get("topic") || "",
  day: initial.get("day") || "",
  topics: [],
  days: [],
  // One digest per loaded day; navigation/reload starts a fresh, bounded timeline.
  timeline: [],
  insight: null,
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
  topbar: $("#topbar"), tabs: $("#tabs"), lang: $("#langToggle"), me: $("#meLink"), catPortal: $("#catPortal"),
  status: $("#status"), digest: $("#digest"), loadMore: $("#loadMore"),
  backToTop: $("#backToTop"),
};
const t = () => TEXT[state.lang];
// Sentinels at the end of groups with more items on the server, and the group each belongs to.
const groupTargets = new WeakMap();
let groupObserver = null;
let activeLoadController = null;

document.addEventListener("DOMContentLoaded", init);

async function init() {
  applyLang();
  el.lang.addEventListener("click", () => {
    const scrollTop = window.scrollY;
    state.lang = state.lang === "zh" ? "en" : "zh";
    storage.setItem("news.lang", state.lang);
    applyLang();
    renderTabs();
    if (state.topic === INSIGHT_ID) renderInsight(state.insight, true, scrollTop);
    else render(true, scrollTop);
  });
  window.addEventListener("popstate", () => {
    const params = new URLSearchParams(location.search);
    state.topic = params.get("topic") || "";
    state.day = state.topic === INSIGHT_ID ? "" : params.get("day") || "";
    renderTabs();
    load();
  });
  setInterval(() => {
    if (document.visibilityState === "visible" && !state.day) load(true);
  }, REFRESH_MS);
  setupScrollControls();
  setupInfiniteScroll();
  setupTopicSwipe();

  // Everything the first screen needs leaves in one round trip: the role cookie replaces the
  // old `/api/me` probe, and the catalogue loads beside the digest instead of before it.
  state.personal = viewerRole() === "user";
  renderTabs();
  await Promise.all([loadCatalog(), load()]);
}

async function fetchJson(path, { signal } = {}) {
  const privatePath = path === "/api/me" || path.startsWith("/api/me/");
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
    return response.json();
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
function viewerRole() {
  const prefix = `${ROLE_COOKIE}=`;
  const entry = document.cookie.split(";").map((part) => part.trim()).find((part) => part.startsWith(prefix));
  return entry ? decodeURIComponent(entry.slice(prefix.length)) : "";
}

// The server clears the cookie on logout; this covers a session that expired on its own.
function clearViewerRole() {
  const secure = location.protocol === "https:" ? "; Secure" : "";
  document.cookie = `${ROLE_COOKIE}=; Path=/; Max-Age=0; SameSite=Lax${secure}`;
}

async function loadCatalog() {
  if (state.personal) return;
  try {
    state.topics = (await fetchJson("/topics")).topics;
    renderTabs();
  } catch {
    // The digest still loads; the tabs just stay at ALL and AI.
  }
}

function search(extra = {}) {
  const params = new URLSearchParams();
  if (state.topic) params.set("topic", state.topic);
  if (state.day) params.set("day", state.day);
  Object.entries(extra).forEach(([key, value]) => params.set(key, value));
  const query = params.toString();
  return query ? `?${query}` : "";
}

function catalogPath() {
  const params = new URLSearchParams();
  if (state.topic) params.set("topic", state.topic);
  params.set("limit", String(DAYS_LIMIT));
  return `/days?${params}`;
}

function dayPath(day) {
  const params = new URLSearchParams({ day, limit: String(PAGE_LIMIT) });
  if (state.topic) params.set("topic", state.topic);
  return `${state.personal ? "/api/me/feed" : "/digest"}?${params}`;
}

function pagePath(day, topic, after) {
  const params = new URLSearchParams({ topic, day, limit: String(PAGE_LIMIT), after });
  return `${state.personal ? "/api/me/feed" : "/digest"}?${params}`;
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
    if (!old
      || group.id !== old.id
      || group.revision !== old.revision
      || group.total !== old.total
      || group.next !== old.next) return true;
    if (group.revision != null) return false;
    const items = group.items || [];
    const oldItems = old.items || [];
    return items.length !== oldItems.length || items.some((item, itemIndex) => item.id !== oldItems[itemIndex]?.id);
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
  groupObserver?.disconnect();
  el.digest.replaceChildren();
}

async function load(silent = false) {
  if (state.topic === INSIGHT_ID) return loadInsight(silent);
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
      const payload = await fetchJson(`/api/me/feed${search({ limit: PAGE_LIMIT })}`, { signal });
      digest = payload.digest;
      days = payload.days;
      if (epoch === state.epoch) state.topics = payload.topics;
    } else {
      const [data, archive] = await Promise.all([
        fetchJson(`/digest${search({ limit: PAGE_LIMIT })}`, { signal }),
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
    if (state.personal && [401, 403].includes(error.status) && epoch === state.epoch) {
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
  const controller = startLoad();
  const { signal } = controller;
  const epoch = ++state.epoch;
  if (state.day) {
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
    const payload = await fetchJson("/insight", { signal });
    if (epoch !== state.epoch) return;
    if (silent
      && payload.insight?.generated_at
      && payload.insight.generated_at === state.insight?.generated_at) {
      state.historyLoading = false;
      state.historyError = false;
      state.historyDone = true;
      setStatus("");
      return;
    }
    state.insight = payload.insight;
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = true;
    setStatus("");
    renderInsight(state.insight, silent);
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
  if (typeof window.IntersectionObserver === "function") {
    const observer = new window.IntersectionObserver(
      (entries) => entries.some((entry) => entry.isIntersecting) && loadOlder(),
      { rootMargin: PRELOAD_MARGIN },
    );
    observer.observe(el.loadMore);
    groupObserver = new window.IntersectionObserver(
      (entries) => entries.forEach((entry) => entry.isIntersecting && loadMoreItems(entry.target)),
      { rootMargin: PRELOAD_MARGIN },
    );
    return;
  }
  window.addEventListener("scroll", () => {
    if (window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - window.innerHeight) loadOlder();
    el.digest.querySelectorAll(".group-more").forEach((sentinel) => {
      if (sentinel.getBoundingClientRect().top < window.innerHeight * 1.7) loadMoreItems(sentinel);
    });
  }, { passive: true });
}

// An empty element after the last card of a group that has more items on the server.
// Scrolling it into view fetches the next page; there is no button and no label.
function groupSentinel(digest, group) {
  const sentinel = document.createElement("div");
  sentinel.className = "group-more";
  sentinel.setAttribute("aria-hidden", "true");
  groupTargets.set(sentinel, { digest, group, loading: false });
  groupObserver?.observe(sentinel);
  return sentinel;
}

// Re-observing reports the current intersection again, so a sentinel that merely moved
// down (or one whose request failed) is re-checked without waiting for a scroll event.
function reobserve(sentinel) {
  groupObserver?.unobserve(sentinel);
  groupObserver?.observe(sentinel);
}

async function loadMoreItems(sentinel) {
  const target = groupTargets.get(sentinel);
  if (!target || target.loading || !sentinel.isConnected || !target.group.next) return;
  const { digest, group } = target;
  if (!Array.isArray(group.items)) group.items = [];
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
      groupObserver?.unobserve(sentinel);
      sentinel.remove();
    }
    window.setTimeout(maybeLoadOlder, 0);
  } catch (error) {
    if (error.name === "AbortError") {
      target.loading = false;
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

// On a phone, a horizontal gesture over the feed moves between topics. Keep the
// tab strip's native horizontal scrolling and article links untouched.
function setupTopicSwipe() {
  if (typeof window.PointerEvent !== "function" || typeof window.matchMedia !== "function") return;
  const mobile = window.matchMedia(MOBILE_QUERY);
  let gesture = null;

  const reset = () => { gesture = null; };
  const isInteractive = (target) => Boolean(target?.closest?.("a, button, input, textarea, select, [contenteditable]"));

  el.digest.addEventListener("pointerdown", (event) => {
    if (!event.isPrimary || event.pointerType !== "touch" || !mobile.matches || isInteractive(event.target)) {
      reset();
      return;
    }
    gesture = { id: event.pointerId, x: event.clientX, y: event.clientY };
  }, { passive: true });

  window.addEventListener("pointerup", (event) => {
    if (!gesture || event.pointerId !== gesture.id) return;
    const { x, y } = gesture;
    reset();
    const dx = event.clientX - x;
    const dy = event.clientY - y;
    if (Math.abs(dx) < SWIPE_MIN_DISTANCE || Math.abs(dx) <= Math.abs(dy) * SWIPE_AXIS_RATIO) return;
    changeTopic(dx < 0 ? 1 : -1);
  }, { passive: true });
  window.addEventListener("pointercancel", (event) => {
    if (gesture?.id === event.pointerId) reset();
  }, { passive: true });
}

function changeTopic(direction) {
  const topicIds = ["", INSIGHT_ID, ...state.topics.map((topic) => topic.id)];
  if (topicIds.length <= 1) return;
  const current = topicIds.indexOf(state.topic);
  const next = ((current < 0 ? 0 : current) + direction + topicIds.length) % topicIds.length;

  el.topbar.classList.remove("is-hidden");
  document.scrollingElement.scrollTop = 0;
  navigate({ topic: topicIds[next], day: "" });
  window.requestAnimationFrame(() => {
    const active = el.tabs.querySelector('[aria-current="true"]');
    active?.scrollIntoView?.({
      behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth",
      block: "nearest",
      inline: "center",
    });
  });
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
  Object.assign(state, changes);
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
  el.catPortal.querySelector(".cat-portal__label").textContent = t().catPortal;
  el.catPortal.setAttribute("aria-label", t().catPortalAria);
  el.catPortal.title = t().catPortal;
  el.backToTop.setAttribute("aria-label", t().backToTop);
  el.backToTop.title = t().backToTop;
  updateHistoryLoader();
}

function renderTabs() {
  // The visual feed gets a wider desktop canvas while every other topic keeps the
  // narrow reading measure. This class also updates when navigating between tabs.
  document.body.classList.toggle("p-topic", state.topic === "p");
  const tabs = [
    { id: "", label: t().all },
    { id: INSIGHT_ID, label: t().insight },
    ...state.topics.map((topic) => ({ id: topic.id, label: name(topic) })),
  ];
  el.tabs.replaceChildren(...tabs.map(({ id, label }) => {
    const tab = button(label, () => navigate({ topic: id, day: "" }), "tab");
    if (id === state.topic) tab.setAttribute("aria-current", "true");
    return tab;
  }));
}

function render(preserveScroll = false, scrollTop = window.scrollY, { latestOnly = false } = {}) {
  if (!state.timeline.length) return;
  if (latestOnly && el.digest.firstElementChild) {
    groupObserver?.disconnect();
    el.digest.firstElementChild.replaceWith(renderDayBlock(state.timeline[0], false));
    updateHistoryLoader();
    window.setTimeout(maybeLoadOlder, 0);
    if (preserveScroll) {
      window.requestAnimationFrame(() => { document.scrollingElement.scrollTop = scrollTop; });
    }
    return;
  }
  // Sentinels of the outgoing blocks must be dropped before the new blocks register theirs.
  groupObserver?.disconnect();
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

function renderInsight(insight, preserveScroll = false, scrollTop = window.scrollY) {
  clearDigest();
  updateHistoryLoader();
  if (!insight) {
    const empty = document.createElement("div");
    empty.className = "empty muted";
    empty.textContent = t().insightEmpty;
    el.digest.append(empty);
    return;
  }
  const section = document.createElement("section");
  section.className = "insight";
  const heading = document.createElement("div");
  heading.className = "insight-heading";
  const title = document.createElement("h2");
  title.textContent = pick(insight, "headline");
  heading.append(title);
  section.append(heading);
  if (insight.cover?.url) section.append(insightCover(insight.cover));
  const overview = document.createElement("p");
  overview.className = "insight-lead";
  overview.append(insightText(pick(insight, "overview")));
  section.append(overview);
  // The server sends only cited news sources, keyed by their original index.
  const sources = new Map((insight.sources || []).map((source) => [source.index, source]));
  const changes = insightParagraph(
    t().changes,
    pick(insight, "changes"),
    "insight-change",
    relativeTime(insight.generated_at),
  );
  const changeSources = sourceList(
    t().sources,
    (insight.changes_source_indexes || []).map((index) => sources.get(index)).filter(Boolean),
  );
  if (changeSources) changes.append(changeSources);
  section.append(changes);
  const researchSources = sourceList(t().researchSources, insight.research_sources || []);
  if (researchSources) section.append(researchSources);
  for (const [label, items, opportunities] of [
    [t().wealth, insight.wealth, true],
    [t().employment, insight.employment, false],
    [t().risks, insight.risks, false],
    [t().actions, insight.actions, false],
  ]) {
    section.append(...renderInsightGroup(label, items || [], sources, opportunities));
  }
  el.digest.append(section);
  if (preserveScroll) {
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      document.scrollingElement.scrollTop = scrollTop;
    }));
  }
}

function insightCover(cover) {
  const figure = document.createElement("figure");
  figure.className = "insight-cover";
  const link = document.createElement("a");
  link.href = cover.source_url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  const image = document.createElement("img");
  image.src = cover.url;
  image.alt = pick(cover, "caption");
  image.loading = "eager";
  image.fetchPriority = "high";
  image.decoding = "async";
  image.referrerPolicy = "no-referrer";
  image.addEventListener("error", () => figure.remove(), { once: true });
  link.append(image);
  const caption = document.createElement("figcaption");
  const source = document.createElement("a");
  source.href = cover.source_url;
  source.target = "_blank";
  source.rel = "noopener noreferrer";
  source.textContent = `${t().coverSource} ↗`;
  caption.append(span(pick(cover, "caption"), "insight-caption"), span(` · ${cover.credit} · `, "muted small"), source);
  figure.append(link, caption);
  return figure;
}

function insightParagraph(label, text, className = "insight-paragraph", metaText = "") {
  const block = document.createElement("div");
  block.className = className;
  const heading = document.createElement("div");
  heading.className = `${className}-heading`;
  const title = document.createElement("h3");
  title.textContent = label;
  heading.append(title);
  if (metaText) heading.append(span(metaText, `${className}-meta muted small`));
  const body = document.createElement("p");
  body.append(insightText(text));
  block.append(heading, body);
  return block;
}

function renderInsightGroup(label, items, sources, opportunities = false) {
  if (!items.length) return [];
  const section = document.createElement("section");
  section.className = "group insight-group";
  const heading = document.createElement("div");
  heading.className = "group-title";
  const title = document.createElement("h2");
  title.textContent = label;
  heading.append(title);
  section.append(heading, ...items.map((item) => insightCard(item, sources, opportunities)));
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
  itemHeading.append(title, span(t().confidence(item.confidence), "muted small"));
  const summary = document.createElement("p");
  summary.className = "item-summary";
  summary.append(insightText(pick(item, "summary")));
  article.append(itemHeading, summary);
  if (opportunity) article.append(opportunityDetails(item));
  const seen = new Set();
  const links = (item.source_indexes || [])
    .map((index) => sources.get(index))
    .filter((source) => source && !seen.has(source.index) && seen.add(source.index));
  const sourceLinks = sourceList(t().sources, links);
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

function sourceLink(url, label, title = "") {
  try {
    const parsed = new URL(url);
    if (!["http:", "https:"].includes(parsed.protocol) || parsed.username || parsed.password) return null;
    const link = document.createElement("a");
    link.href = parsed.href;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.title = title;
    link.textContent = label;
    return link;
  } catch {
    return null;
  }
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

function sourceList(label, links) {
  const seen = new Set();
  const anchors = links.map((source) => sourceLink(
    source.url,
    `${source.source || hostOf(source.url)} ↗`,
    pick(source, "title"),
  )).filter((link) => link && !seen.has(link.href) && seen.add(link.href));
  if (!anchors.length) return null;
  const list = document.createElement("div");
  list.className = "insight-sources";
  list.append(span(label, "muted small"), ...anchors);
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
  block.dataset.day = digest.day || "";
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
    section.classList.toggle("image-news-group", group.id === "p");
    if (!single) {
      const heading = document.createElement("div");
      heading.className = "group-title";
      const title = document.createElement("h2");
      title.textContent = name(group);
      const count = document.createElement("p");
      count.className = "muted small";
      count.textContent = t().items(group.total);
      heading.append(title, count);
      section.append(heading);
    }
    section.append(...(group.items || []).map((item) => renderGroupItem(group, item)));
    if (group.next) section.append(groupSentinel(digest, group));
    return section;
  });
}

function renderGroupItem(group, item) {
  return group.id === "p" ? imageNewsCard(item) : card(item);
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
  const caption = pick(item.image, "caption");
  if (caption) {
    const credit = item.image.credit ? ` · ${item.image.credit}` : "";
    const note = span(`${caption}${credit}`, "image-news-caption");
    article.insertBefore(note, article.querySelector(".item-meta"));
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

function card(item) {
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
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.textContent = pick(item, "title");
  title.append(link);

  const summary = document.createElement("p");
  summary.className = "item-summary";
  summary.textContent = pick(item, "summary");

  article.append(meta, title, summary);
  const tags = state.lang === "en" && item.tags_en?.length ? item.tags_en : item.tags;
  if (tags.length) {
    const tagList = document.createElement("div");
    tagList.className = "item-tags";
    tagList.append(...tags.map((tag) => span(tag, "tag")));
    article.append(tagList);
  }
  return article;
}

// The bar slides away while reading down and returns on the first upward scroll, so the
// list gets the whole screen without losing the controls.
function setupScrollControls() {
  let last = window.scrollY;
  let backToTopVisible = false;
  let scrollAnchor = last;
  const update = () => {
    const y = Math.max(0, window.scrollY);
    if (y <= 48 || Math.abs(y - last) > 4) el.topbar.classList.toggle("is-hidden", y > last && y > 48);
    // Track the turning point so slow scrolling counts without reacting to tiny reversals.
    scrollAnchor = backToTopVisible ? Math.min(scrollAnchor, y) : Math.max(scrollAnchor, y);
    if (y <= 100) {
      backToTopVisible = false;
      scrollAnchor = y;
    } else if (Math.abs(y - scrollAnchor) >= 8) {
      backToTopVisible = y < scrollAnchor && y > window.innerHeight;
      scrollAnchor = y;
    }
    el.backToTop.classList.toggle("is-visible", backToTopVisible);
    last = y;
  };
  window.addEventListener("scroll", update, { passive: true });
  window.addEventListener("resize", update);
  window.addEventListener("pageshow", update);
  el.backToTop.addEventListener("click", () => {
    el.topbar.classList.remove("is-hidden");
    el.topbar.querySelector(".brand").focus({ preventScroll: true });
    // The existing CSS handles smooth scrolling and reduced-motion preferences.
    window.scrollTo({ top: 0, behavior: "auto" });
  });
  update();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const name = (topic) => (state.lang === "en" && topic.name_en) || topic.name;
const pick = (item, field) => (state.lang === "en" && item[`${field}_en`]) || item[field];
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
