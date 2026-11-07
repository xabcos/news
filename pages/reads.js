"use strict";

PublicPage.page();

const english = document.documentElement.lang === "en";

// 分享 counts its reads on this site, so cards that came from a cache ask for the counts as they
// are now. Reads' counts come from Stats every half hour and stay as the page has them.
function freshen(cards) {
  const holders = cards.flatMap((card) => [...card.querySelectorAll("[data-views]")]);
  for (let start = 0; start < holders.length; start += 50) {
    const batch = holders.slice(start, start + 50);
    const ids = batch.map((holder) => holder.dataset.views).join(",");
    fetch(`/api/articles/views?ids=${ids}`)
      .then((response) => (response.ok ? response.json() : null))
      .then((data) => {
        for (const holder of batch) {
          const views = data?.views?.[holder.dataset.views];
          if (!Number.isSafeInteger(views)) continue;
          holder.querySelector(".read-views > span").textContent = views;
          holder.hidden = views < 1;
        }
      })
      .catch(() => {});
  }
}

(() => {
  const list = document.querySelector(".reads-list");
  const pagination = document.querySelector(".reads-pagination");
  if (!list || !pagination) return;

  const nextLink = pagination.querySelector('[rel="next"]');
  let next = nextLink?.getAttribute("href") || "";
  // The browser's back/forward cache normally keeps the whole list. Restore this
  // history entry as well when the browser has had to discard the document.
  const saved = history.state?.reads;
  if (performance.getEntriesByType("navigation")[0]?.type === "back_forward"
    && saved?.url === location.href) {
    list.innerHTML = saved.html;
    next = saved.next;
    window.requestAnimationFrame(() => window.scrollTo({ top: saved.y, behavior: "instant" }));
  }
  if (!next && !saved) return;

  const seen = new Set([...list.querySelectorAll(".item-title a")].map((link) => link.getAttribute("href")));
  const loader = document.createElement("div");
  loader.className = "load-more";
  loader.setAttribute("role", "status");
  loader.setAttribute("aria-live", "polite");
  list.after(loader);
  if (nextLink) nextLink.hidden = true;
  pagination.hidden = !pagination.querySelector('[rel="prev"]');

  let controller = null;
  let observer = null;
  let failed = false;
  let suspended = false;

  function check() {
    if (next && !controller && !failed && !suspended && !document.hidden
      && loader.getBoundingClientRect().top < window.innerHeight * 2) loadNext();
  }

  function status() {
    loader.replaceChildren();
    if (failed) {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "button button-quiet button-small";
      retry.textContent = english ? "Retry" : "重试";
      retry.addEventListener("click", () => {
        failed = false;
        loadNext();
      });
      loader.append(english ? "Failed to load " : "加载失败 ", retry);
    } else if (!next) {
      loader.textContent = english ? "You've reached the end." : "已读完全部文章";
    }
  }

  async function loadNext() {
    if (!next || controller || failed || suspended) return;
    const request = new AbortController();
    controller = request;
    const timeout = window.setTimeout(() => request.abort(), 20_000);
    list.setAttribute("aria-busy", "true");
    status();
    try {
      // Reuse the ordinary HTML pagination; only cards are imported, so fetching
      // a page never executes its scripts or records another page view.
      const response = await fetch(PublicPage.readPath(next), { signal: request.signal });
      if (response.status === 404) {
        if (controller === request) next = "";
        return;
      }
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
      if (controller !== request) return;
      const cards = [...page.querySelectorAll(".reads-list > .item")];
      if (!cards.length) throw new Error("Missing Reads list");
      const added = cards.filter((card) => {
        const href = card.querySelector(".item-title a")?.getAttribute("href");
        if (!href || seen.has(href)) return false;
        seen.add(href);
        return true;
      });
      list.append(...added);
      freshen(added);
      next = page.querySelector('.reads-pagination [rel="next"]')?.getAttribute("href") || "";
    } catch {
      if (controller === request) failed = true;
    } finally {
      window.clearTimeout(timeout);
      if (controller === request) {
        controller = null;
        list.removeAttribute("aria-busy");
        status();
        if (!next) observer?.disconnect();
        // A short page or a tall viewport can still need another page without
        // a new intersection or scroll event.
        window.requestAnimationFrame(check);
      }
    }
  }

  // The margin is one window height, the distance `check` allows, so a new height needs a new
  // observer. (A percentage would count the window's width.)
  function observe() {
    observer?.disconnect();
    observer = new IntersectionObserver(check, { rootMargin: `0px 0px ${window.innerHeight}px 0px` });
    if (next) observer.observe(loader);
    check();
  }
  window.addEventListener("resize", observe);
  document.addEventListener("visibilitychange", check);
  window.addEventListener("pageshow", () => {
    suspended = false;
    check();
  });
  window.addEventListener("pagehide", () => {
    suspended = true;
    controller?.abort();
    controller = null;
    list.removeAttribute("aria-busy");
    try {
      history.replaceState({ ...history.state, reads: {
        url: location.href, html: list.innerHTML, next, y: window.scrollY,
      } }, "");
    } catch { /* Native back/forward restoration still works if history storage is full. */ }
  });
  status();
  window.requestAnimationFrame(observe);
})();

// Lists shown again from the back/forward cache ask too.
freshen([...document.querySelectorAll(".reads-list")]);
window.addEventListener("pageshow", (event) => {
  if (event.persisted) freshen([...document.querySelectorAll(".reads-list")]);
});

// 调研's question box. A question goes out under its reader's pen name, so it needs a session and
// a pen name: a reader without them is told where to get them, and the question waits in this
// browser until they come back. Pro readers may keep the article to themselves.
(() => {
  const form = document.querySelector(".research-ask");
  if (!form) return;
  const KEPT = "news.research.question";
  const input = form.elements.question;
  const button = form.querySelector("button");
  const choice = form.querySelector(".research-public");
  const told = Object.assign(document.createElement("div"), { className: "research-told", hidden: true });
  told.setAttribute("role", "status");
  form.append(told);
  try { input.value ||= localStorage.getItem(KEPT) || ""; } catch { /* The box works without storage. */ }
  if (PublicPage.role()) {
    fetch("/api/research")
      .then((response) => (response.ok ? response.json() : null))
      .then((data) => { choice.hidden = !data?.private; })
      .catch(() => {});
  }
  const workspace = (text) => Object.assign(document.createElement("a"), { href: "/me/", textContent: text });
  const say = (...parts) => {
    told.replaceChildren(...parts);
    told.hidden = false;
  };
  input.addEventListener("input", () => { told.hidden = true; });
  document.addEventListener("click", (event) => {
    if (!form.contains(event.target)) told.hidden = true;
  });
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const question = input.value.trim();
    if (!question) return;
    try { localStorage.setItem(KEPT, question); } catch { /* The box works without storage. */ }
    button.disabled = true;
    try {
      const response = await fetch("/api/research", {
        method: "POST",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ question, public: choice.hidden || choice.querySelector("input").checked }),
      });
      const data = await response.json().catch(() => null);
      if (response.status === 401) {
        return say(english
          ? "Research goes out under your pen name. Sign in to the workspace and set one first; your question stays here. "
          : "调研以你的笔名公开发布，先登录工作台、设置笔名吧，问题已经帮你留着。", workspace(english ? "Workspace" : "登录工作台"));
      }
      if (!response.ok) {
        const message = data?.error?.message || `HTTP ${response.status}`;
        // A pen name to set, or Pro to have, waits in the workspace.
        return say(message, ...([403, 409].includes(response.status) ? [" ", workspace(english ? "Workspace" : "去工作台")] : []));
      }
      // Whatever was typed meanwhile stays.
      try {
        if (localStorage.getItem(KEPT) === question) localStorage.removeItem(KEPT);
      } catch { /* The box works without storage. */ }
      if (input.value.trim() === question) input.value = "";
      // A writer may be free for it at once; else it waits behind others, or comes next.
      const standing = data.now ? (english ? "Starting now" : "马上开始调研")
        : data.ahead ? (english ? `Queued with ${data.ahead} ahead` : `已排进调研队列，前面还有 ${data.ahead} 位`)
          : (english ? "Queued, next in line" : "已排进调研队列，下一个就写它");
      say(english
        ? `${standing}: ready in about ${data.minutes} minutes. Follow it in your `
        : `${standing}，大约 ${data.minutes} 分钟后写好，进度在`, workspace(english ? "workspace" : "工作台"), english ? "." : "里看。");
    } catch {
      say(english ? "Couldn't send it. Try again." : "没有发出去，请重试。");
    } finally {
      button.disabled = false;
    }
  });
})();
