"use strict";

const $ = (selector, root = document) => root.querySelector(selector);
const REFRESH_MS = 5 * 60 * 1000;
const PAGE_LIMIT = 100;
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
    locale: "zh-CN", lang: "EN", all: "全部", today: "今天", updated: "更新于", top: "重要",
    fresh: "NEW", loading: "加载中…", failed: "加载失败", retry: "重试",
    empty: "这一天还没有内容", latest: "查看最近有内容的一天", prev: "前一天", next: "后一天",
    personal: "我的订阅", space: "我的空间", personalEmpty: "还没有可阅读的内容", manageTopics: "管理我的主题",
    items: (n) => `${n} 条`, note: ["由 ", " 提供支持"],
  },
  en: {
    locale: "en", lang: "中文", all: "All", today: "Today", updated: "Updated", top: "Top",
    fresh: "NEW", loading: "Loading…", failed: "Failed to load", retry: "Retry",
    empty: "Nothing collected for this day yet", latest: "Go to the latest day with content",
    prev: "Previous day", next: "Next day",
    personal: "My feed", space: "My space", personalEmpty: "Nothing to read yet", manageTopics: "Manage my topics",
    items: (n) => `${n} ${n === 1 ? "item" : "items"}`, note: ["Powered by ", ""],
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
  viewer: null,
  personal: false,
  // Items first collected after the previous visit get a NEW mark.
  lastVisit: Number(storage.getItem("news.lastVisit")) || Date.now(),
  epoch: 0,
};
storage.setItem("news.lastVisit", String(Date.now()));

const el = {
  topbar: $("#topbar"), tabs: $("#tabs"), lang: $("#langToggle"), me: $("#meLink"), space: $("#spaceLink"), scope: $("#feedScope"),
  prev: $("#prevDay"), next: $("#nextDay"),
  title: $("#dayTitle"), meta: $("#dayMeta"), status: $("#status"), digest: $("#digest"), note: $("#footNote"),
};
const t = () => TEXT[state.lang];

document.addEventListener("DOMContentLoaded", init);

async function init() {
  applyLang();
  el.lang.addEventListener("click", () => {
    state.lang = state.lang === "zh" ? "en" : "zh";
    storage.setItem("news.lang", state.lang);
    applyLang();
    renderTabs();
    render();
  });
  el.prev.addEventListener("click", () => step(1));
  el.next.addEventListener("click", () => step(-1));
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
      state.viewer = viewer;
      state.personal = true;
    }
  } catch (error) {
    // A missing session is the normal anonymous path. Other failures should not make
    // the public page unavailable; the public digest remains a useful fallback.
    if (![401, 403].includes(error.status)) console.debug("viewer lookup failed", error);
  }
  renderScope();
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

async function load(silent = false) {
  const epoch = ++state.epoch;
  if (!silent) setStatus(t().loading);
  try {
    let digest;
    let days;
    if (state.personal) {
      const payload = await fetchJson(`/api/me/feed${search({ limit: PAGE_LIMIT })}`);
      digest = payload?.digest || payload;
      days = payload?.days || [];
      if (epoch === state.epoch && Array.isArray(payload?.topics)) state.topics = payload.topics;
    } else {
      const topicQuery = state.topic ? `?topic=${encodeURIComponent(state.topic)}` : "";
      [digest, days] = await Promise.all([
        fetchJson(`/digest${search({ limit: PAGE_LIMIT })}`),
        fetchJson(`/days${topicQuery}`),
      ]);
    }
    if (epoch !== state.epoch) return;
    state.days = (Array.isArray(days) ? days : days?.days || []).map((entry) =>
      typeof entry === "string" ? entry : entry?.day,
    ).filter(Boolean);
    if (state.personal) renderTabs();
    // A background refresh only repaints when something actually changed.
    if (silent && JSON.stringify(digest.topics) === JSON.stringify(state.digest?.topics)) return;
    state.digest = digest;
    setStatus("");
    render();
  } catch (error) {
    // If a session expires while the page is open, switch back to the public feed
    // instead of leaving a private page permanently stuck on an auth error.
    if (state.personal && [401, 403].includes(error.status) && epoch === state.epoch) {
      state.personal = false;
      state.viewer = null;
      state.topic = "";
      state.day = "";
      state.topics = [];
      state.days = [];
      state.digest = null;
      renderScope();
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
      history.replaceState({}, "", `${location.pathname}${search()}`);
      await load(silent);
      return;
    }
    if (epoch === state.epoch && !silent) setStatus(t().failed, true);
  }
}

function navigate(changes) {
  Object.assign(state, changes);
  history.pushState({}, "", `${location.pathname}${search()}`);
  renderTabs();
  load();
}

// state.days is newest first: +1 walks back in time, -1 forward.
function step(direction) {
  const current = state.digest?.day;
  const index = state.days.indexOf(current);
  const target = index < 0 ? (direction > 0 ? state.days[0] : undefined) : state.days[index + direction];
  if (!target) return;
  navigate({ day: target === state.days[0] ? "" : target });
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
  if (el.space) el.space.textContent = t().space;
  if (el.scope) el.scope.textContent = t().personal;
  el.prev.setAttribute("aria-label", t().prev);
  el.next.setAttribute("aria-label", t().next);
  const [before, after] = t().note;
  const xai = document.createElement("a");
  xai.href = "https://xairouter.com";
  xai.rel = "noopener noreferrer";
  xai.textContent = "XAI";
  el.note.replaceChildren(before, xai, after);
}

function renderScope() {
  document.body.classList.toggle("is-personal-feed", state.personal);
  el.scope?.classList.toggle("hidden", !state.personal);
  el.space?.classList.toggle("hidden", !state.personal);
  if (el.me) el.me.textContent = t().space;
}

function renderTabs() {
  const tabs = [{ id: "", label: t().all }, ...state.topics.map((topic) => ({ id: topic.id, label: name(topic) }))];
  el.tabs.replaceChildren(...tabs.map(({ id, label }) => {
    const tab = button(label, () => navigate({ topic: id, day: "" }), "tab");
    if (id === state.topic) tab.setAttribute("aria-current", "true");
    return tab;
  }));
}

function render() {
  const digest = state.digest;
  if (!digest) return;
  const groups = digest.topics;
  const total = groups.reduce((sum, group) => sum + group.total, 0);
  const updated = groups.map((group) => group.updated_at).filter(Boolean).sort().pop();

  el.title.textContent = digest.day ? formatDay(digest.day, digest.timezone) : "NEWS";
  el.meta.textContent = digest.day
    ? [updated && `${t().updated} ${formatTime(updated, digest.timezone)}`, t().items(total)].filter(Boolean).join(" · ")
    : "";
  const index = state.days.indexOf(digest.day);
  el.prev.disabled = !(index < 0 ? state.days.length : state.days[index + 1]);
  el.next.disabled = index <= 0;

  if (!total) {
    el.digest.replaceChildren(emptyState(groups));
    return;
  }
  const single = groups.length === 1;
  el.digest.replaceChildren(...groups.filter((group) => group.items.length).map((group) => {
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
    section.append(...group.items.map(card));
    return section;
  }));
}

function emptyState(groups) {
  const box = document.createElement("div");
  box.className = "empty muted";
  box.append(state.personal ? t().personalEmpty : t().empty);
  if (state.personal && !state.topics.length) {
    box.append(document.createElement("br"));
    const manage = document.createElement("a");
    manage.className = "button button-secondary button-small";
    manage.href = "/me/";
    manage.textContent = t().manageTopics;
    box.append(manage);
  }
  const latest = groups.map((group) => group.latest_day).filter(Boolean).sort().pop();
  if (latest && latest !== state.digest.day) {
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
  meta.append(span(item.source || hostOf(item.url), "item-source"), "·", span(relativeTime(item.published_at || item.first_seen_at), "item-time"));
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

function formatTime(iso, timeZone) {
  return new Intl.DateTimeFormat(t().locale, { hour: "2-digit", minute: "2-digit", timeZone }).format(new Date(iso));
}

function relativeTime(iso) {
  const minutes = Math.round((Date.parse(iso) - Date.now()) / 60000);
  const format = new Intl.RelativeTimeFormat(t().locale, { numeric: "auto" });
  if (Math.abs(minutes) < 60) return format.format(minutes, "minute");
  if (Math.abs(minutes) < 60 * 24) return format.format(Math.round(minutes / 60), "hour");
  if (Math.abs(minutes) < 60 * 24 * 7) return format.format(Math.round(minutes / 1440), "day");
  return new Intl.DateTimeFormat(t().locale, { month: "short", day: "numeric" }).format(new Date(iso));
}
