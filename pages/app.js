"use strict";

const $ = (selector, root = document) => root.querySelector(selector);
const REFRESH_MS = 5 * 60 * 1000;
const PAGE_LIMIT = 100;
// Keep the simple timeline bounded to the archive window exposed by the API.
const DAYS_LIMIT = 366;
const PRELOAD_MARGIN = "0px 0px 70% 0px";
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
    locale: "zh-CN", lang: "EN", all: "全部", today: "今天", top: "重要",
    fresh: "NEW", loading: "加载中…", loadingOlder: "加载更早内容…", failed: "加载失败", retry: "重试",
    empty: "这一天还没有内容", latest: "查看最近有内容的一天",
    space: "我的空间", personalEmpty: "还没有可阅读的内容", manageTopics: "管理我的主题",
    items: (n) => `${n} 条`,
  },
  en: {
    locale: "en", lang: "中文", all: "All", today: "Today", top: "Top",
    fresh: "NEW", loading: "Loading…", loadingOlder: "Loading older news…", failed: "Failed to load", retry: "Retry",
    empty: "Nothing collected for this day yet", latest: "Go to the latest day with content",
    space: "My space", personalEmpty: "Nothing to read yet", manageTopics: "Manage my topics",
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
  digest: null,
  // One digest per loaded day; navigation/reload starts a fresh, bounded timeline.
  timeline: [],
  personal: false,
  historyLoading: false,
  historyError: false,
  historyDone: false,
  historyObserver: null,
  // Items first collected after the previous visit get a NEW mark.
  lastVisit: Number(storage.getItem("news.lastVisit")) || Date.now(),
  epoch: 0,
};
storage.setItem("news.lastVisit", String(Date.now()));

const el = {
  topbar: $("#topbar"), tabs: $("#tabs"), lang: $("#langToggle"), me: $("#meLink"),
  status: $("#status"), digest: $("#digest"), loadMore: $("#loadMore"),
};
const t = () => TEXT[state.lang];

document.addEventListener("DOMContentLoaded", init);

async function init() {
  applyLang();
  el.lang.addEventListener("click", () => {
    const scrollTop = window.scrollY;
    state.lang = state.lang === "zh" ? "en" : "zh";
    storage.setItem("news.lang", state.lang);
    applyLang();
    renderTabs();
    render(true, scrollTop);
  });
  window.addEventListener("popstate", () => {
    const params = new URLSearchParams(location.search);
    state.topic = params.get("topic") || "";
    state.day = params.get("day") || "";
    renderTabs();
    load();
  });
  setInterval(() => {
    if (document.visibilityState === "visible" && !state.day) load(true);
  }, REFRESH_MS);
  autoHideTopbar();
  setupInfiniteScroll();

  await resolveViewer();
  await loadCatalog();
  renderTabs();
  await load();
}

async function fetchJson(path) {
  const privatePath = path === "/api/me" || path.startsWith("/api/me/");
  const response = await fetch(path, {
    credentials: "same-origin",
    cache: privatePath ? "no-store" : "default",
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    const error = new Error(`HTTP ${response.status}`);
    error.status = response.status;
    throw error;
  }
  return response.json();
}

// The public page is cacheable, but the selected account feed is always fetched with the
// session cookie and never allowed to fall into a shared browser cache.
async function resolveViewer() {
  try {
    const data = await fetchJson("/api/me");
    const viewer = data?.user || data;
    if (viewer?.email && String(viewer.role || "user").toLowerCase() !== "admin") {
      state.personal = true;
    }
  } catch (error) {
    // A missing session is the normal anonymous path. Other failures should not make
    // the public page unavailable; the public digest remains a useful fallback.
    if (![401, 403].includes(error.status)) console.debug("viewer lookup failed", error);
  }
}

async function loadCatalog() {
  if (state.personal) return;
  try {
    state.topics = (await fetchJson("/topics")).topics || [];
  } catch {
    // The digest still loads; the tabs just stay empty.
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

function normalizeDays(payload) {
  const values = Array.isArray(payload) ? payload : payload?.days || [];
  return values
    .map((entry) => typeof entry === "string" ? entry : entry?.day)
    .filter((day, index, list) => day && list.indexOf(day) === index);
}

async function load(silent = false) {
  const epoch = ++state.epoch;
  if (!silent) {
    state.digest = null;
    state.timeline = [];
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = false;
    el.digest.replaceChildren();
    updateHistoryLoader();
    setStatus(t().loading);
  }
  try {
    let digest;
    let days;
    if (state.personal) {
      const payload = await fetchJson(`/api/me/feed${search({ limit: PAGE_LIMIT })}`);
      digest = payload?.digest || payload;
      days = payload?.days || [];
      if (epoch === state.epoch && Array.isArray(payload?.topics)) state.topics = payload.topics;
    } else {
      [digest, days] = await Promise.all([
        fetchJson(`/digest${search({ limit: PAGE_LIMIT })}`),
        fetchJson(catalogPath()),
      ]);
    }
    if (epoch !== state.epoch) return;
    state.days = normalizeDays(days);
    if (state.personal) renderTabs();
    // A background refresh only repaints when something actually changed.
    if (silent
      && digest?.day === state.digest?.day
      && JSON.stringify(digest?.topics) === JSON.stringify(state.digest?.topics)) {
      updateHistoryBoundary();
      setStatus("");
      updateHistoryLoader();
      return;
    }
    if (silent && state.timeline.length) {
      if (state.timeline[0].day === digest?.day) state.timeline[0] = { day: digest?.day || "", digest };
      else state.timeline.unshift({ day: digest?.day || "", digest });
    } else {
      state.timeline = [{ day: digest?.day || "", digest }];
    }
    state.digest = state.timeline[0].digest;
    state.historyError = false;
    updateHistoryBoundary();
    setStatus("");
    render(silent);
  } catch (error) {
    // If a session expires while the page is open, switch back to the public feed
    // instead of leaving a private page permanently stuck on an auth error.
    if (state.personal && [401, 403].includes(error.status) && epoch === state.epoch) {
      state.personal = false;
      state.topic = "";
      state.day = "";
      state.topics = [];
      state.days = [];
      state.digest = null;
      state.timeline = [];
      state.historyDone = false;
      state.historyError = false;
      history.replaceState({}, "", `${location.pathname}${search()}`);
      await loadCatalog();
      renderTabs();
      await load(silent);
      return;
    }
    // A link can still contain a topic outside this account's visible catalogue (for
    // example, another user's private topic). Drop that selector instead of leaving the
    // page on a confusing 404.
    if (state.personal && error.status === 404 && state.topic && epoch === state.epoch) {
      state.topic = "";
      state.day = "";
      state.timeline = [];
      state.digest = null;
      history.replaceState({}, "", `${location.pathname}${search()}`);
      await load(silent);
      return;
    }
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
  state.historyLoading = true;
  state.historyError = false;
  updateHistoryLoader();
  try {
    const payload = await fetchJson(dayPath(day));
    if (timeline !== state.timeline) return;
    const digest = payload?.digest || payload;
    if (!digest?.day || state.timeline.some((entry) => entry.day === digest.day)) {
      state.historyDone = true;
      return;
    }
    state.timeline.push({ day: digest.day, digest });
    el.digest.append(renderDayBlock(digest, state.timeline.length - 1));
    updateHistoryBoundary();
  } catch {
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
  if (!el.loadMore || state.historyLoading || state.historyError || state.historyDone || !state.timeline.length) return;
  const remaining = document.documentElement.scrollHeight - window.scrollY - window.innerHeight;
  if (remaining < window.innerHeight * .7) loadOlder();
}

function setupInfiniteScroll() {
  if (!el.loadMore) return;
  if (typeof window.IntersectionObserver === "function") {
    state.historyObserver = new window.IntersectionObserver(
      (entries) => entries.some((entry) => entry.isIntersecting) && loadOlder(),
      { rootMargin: PRELOAD_MARGIN },
    );
    state.historyObserver.observe(el.loadMore);
    return;
  }
  window.addEventListener("scroll", () => {
    if (window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - window.innerHeight) loadOlder();
  }, { passive: true });
}

function updateHistoryLoader() {
  if (!el.loadMore) return;
  el.loadMore.replaceChildren();
  if (state.historyLoading) {
    el.loadMore.append(t().loadingOlder);
  } else if (state.historyError) {
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
  if (el.me) el.me.textContent = t().space;
  updateHistoryLoader();
}

function renderTabs() {
  const tabs = [{ id: "", label: t().all }, ...state.topics.map((topic) => ({ id: topic.id, label: name(topic) }))];
  el.tabs.replaceChildren(...tabs.map(({ id, label }) => {
    const tab = button(label, () => navigate({ topic: id, day: "" }), "tab");
    if (id === state.topic) tab.setAttribute("aria-current", "true");
    return tab;
  }));
}

function render(preserveScroll = false, scrollTop = window.scrollY) {
  const digest = state.timeline[0]?.digest || state.digest;
  if (!digest) return;
  state.digest = digest;
  const timeline = state.timeline.length ? state.timeline : [{ day: digest.day || "", digest }];
  el.digest.replaceChildren(...timeline.map((entry) => renderDayBlock(entry.digest)));
  updateHistoryLoader();
  if (preserveScroll) {
    // Off-screen blocks settle after the first layout pass; restore after two paints.
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      document.scrollingElement.scrollTop = scrollTop;
    }));
  }
  window.setTimeout(maybeLoadOlder, 0);
}

function renderDayBlock(digest) {
  const block = document.createElement("section");
  block.className = "day-block";
  block.dataset.day = digest.day || "";
  if (digest.day) {
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
    section.append(...(group.items || []).map(card));
    return section;
  });
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
  if (item.tags.length) {
    const tags = document.createElement("div");
    tags.className = "item-tags";
    tags.append(...item.tags.map((tag) => span(tag, "tag")));
    article.append(tags);
  }
  return article;
}

// The bar slides away while reading down and returns on the first upward scroll, so the
// list gets the whole screen without losing the controls.
function autoHideTopbar() {
  let last = window.scrollY;
  window.addEventListener("scroll", () => {
    const y = Math.max(0, window.scrollY);
    if (Math.abs(y - last) > 4) el.topbar.classList.toggle("is-hidden", y > last && y > 48);
    last = y;
  }, { passive: true });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const name = (topic) => (state.lang === "en" && topic.name_en) || topic.name;
const pick = (item, field) => (state.lang === "en" && item[`${field}_en`]) || item[field];

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
  return new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(new Date());
}

function formatDay(day, timeZone) {
  const [year, month, date] = day.split("-").map(Number);
  const label = new Intl.DateTimeFormat(t().locale, {
    year: year === new Date().getFullYear() ? undefined : "numeric",
    month: "long",
    day: "numeric",
    weekday: "long",
  }).format(new Date(year, month - 1, date));
  return day === todayIn(timeZone) ? `${t().today} · ${label}` : label;
}

function relativeTime(iso) {
  const minutes = Math.round((Date.parse(iso) - Date.now()) / 60000);
  const format = new Intl.RelativeTimeFormat(t().locale, { numeric: "auto" });
  if (Math.abs(minutes) < 60) return format.format(minutes, "minute");
  if (Math.abs(minutes) < 60 * 24) return format.format(Math.round(minutes / 60), "hour");
  if (Math.abs(minutes) < 60 * 24 * 7) return format.format(Math.round(minutes / 1440), "day");
  return new Intl.DateTimeFormat(t().locale, { month: "short", day: "numeric" }).format(new Date(iso));
}
