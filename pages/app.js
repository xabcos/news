"use strict";

const $ = (selector) => document.querySelector(selector);
const REFRESH_MS = 5 * 60 * 1000;
const PAGE_LIMIT = 128;
// Keep the simple timeline bounded to the archive window exposed by the API.
const DAYS_LIMIT = 366;
const PRELOAD_MARGIN = "0px 0px 70% 0px";
const SWIPE_MIN_DISTANCE = 48;
const SWIPE_AXIS_RATIO = 1.2;
const MOBILE_QUERY = "(max-width: 768px)";
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
    fresh: "NEW", failed: "加载失败", retry: "重试",
    empty: "这一天还没有内容", latest: "查看最近有内容的一天",
    space: "工作台", personalEmpty: "还没有可阅读的内容", manageTopics: "管理我的主题",
    backToTop: "回到顶部",
    items: (n) => `${n} 条`,
  },
  en: {
    locale: "en", lang: "中文", all: "All", today: "Today", top: "Top",
    fresh: "NEW", failed: "Failed to load", retry: "Retry",
    empty: "Nothing collected for this day yet", latest: "Go to the latest day with content",
    space: "Workspace", personalEmpty: "Nothing to read yet", manageTopics: "Manage my topics",
    backToTop: "Back to top",
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
  topbar: $("#topbar"), tabs: $("#tabs"), lang: $("#langToggle"), me: $("#meLink"),
  status: $("#status"), digest: $("#digest"), loadMore: $("#loadMore"),
  backToTop: $("#backToTop"),
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
  setupScrollControls();
  setupInfiniteScroll();
  setupTopicSwipe();

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
    const { user } = await fetchJson("/api/me");
    state.personal = user.role === "user";
  } catch (error) {
    // A missing session is the normal anonymous path. Other failures should not make
    // the public page unavailable; the public digest remains a useful fallback.
    if (![401, 403].includes(error.status)) console.debug("viewer lookup failed", error);
  }
}

async function loadCatalog() {
  if (state.personal) return;
  try {
    state.topics = (await fetchJson("/topics")).topics;
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

async function load(silent = false) {
  const epoch = ++state.epoch;
  if (!silent) {
    state.timeline = [];
    state.historyLoading = false;
    state.historyError = false;
    state.historyDone = false;
    el.digest.replaceChildren();
    updateHistoryLoader();
    setStatus("");
  }
  try {
    let digest;
    let days;
    if (state.personal) {
      const payload = await fetchJson(`/api/me/feed${search({ limit: PAGE_LIMIT })}`);
      digest = payload.digest;
      days = payload.days;
      if (epoch === state.epoch) state.topics = payload.topics;
    } else {
      const [data, archive] = await Promise.all([
        fetchJson(`/digest${search({ limit: PAGE_LIMIT })}`),
        fetchJson(catalogPath()),
      ]);
      digest = data;
      days = archive.days;
    }
    if (epoch !== state.epoch) return;
    state.days = days.map((entry) => entry.day);
    if (state.personal) renderTabs();
    // A background refresh only repaints when something actually changed.
    const current = state.timeline[0];
    if (silent
      && digest.day === current?.day
      && JSON.stringify(digest.topics) === JSON.stringify(current?.topics)) {
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
  const personal = state.personal;
  state.historyLoading = true;
  state.historyError = false;
  updateHistoryLoader();
  try {
    const payload = await fetchJson(dayPath(day));
    if (timeline !== state.timeline) return;
    const digest = personal ? payload.digest : payload;
    if (!digest?.day || state.timeline.some((entry) => entry.day === digest.day)) {
      state.historyDone = true;
      return;
    }
    state.timeline.push(digest);
    el.digest.append(renderDayBlock(digest, true));
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
    return;
  }
  window.addEventListener("scroll", () => {
    if (window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - window.innerHeight) loadOlder();
  }, { passive: true });
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
  const topicIds = ["", ...state.topics.map((topic) => topic.id)];
  const current = topicIds.indexOf(state.topic);
  const next = (current < 0 ? 0 : current) + direction;
  if (next < 0 || next >= topicIds.length) return;

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
  el.backToTop.setAttribute("aria-label", t().backToTop);
  el.backToTop.title = t().backToTop;
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
  if (!state.timeline.length) return;
  el.digest.replaceChildren(...state.timeline.map((digest, index) => {
    const day = digest.day;
    const showDayHeading = index > 0 || !day || day !== todayIn(digest.timezone);
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
