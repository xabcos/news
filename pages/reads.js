"use strict";

PublicPage.init();

const english = document.documentElement.lang === "en";
try { localStorage.setItem("news.lang", english ? "en" : "zh"); } catch { /* Reading works without storage. */ }

const activeTab = document.querySelector('.tabs [aria-current="true"]');
activeTab?.scrollIntoView({ block: "nearest", inline: "center" });

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
      const response = await fetch(next, { signal: request.signal });
      if (response.status === 404) {
        if (controller === request) next = "";
        return;
      }
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
      if (controller !== request) return;
      const cards = [...page.querySelectorAll(".reads-list > .item")];
      if (!cards.length) throw new Error("Missing Reads list");
      list.append(...cards.filter((card) => {
        const href = card.querySelector(".item-title a")?.getAttribute("href");
        if (!href || seen.has(href)) return false;
        seen.add(href);
        return true;
      }));
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

  function observe() {
    if (typeof window.IntersectionObserver === "function") {
      observer?.disconnect();
      observer = new IntersectionObserver(check, { rootMargin: `0px 0px ${window.innerHeight}px 0px` });
      if (next) observer.observe(loader);
    }
    check();
  }
  if (typeof window.IntersectionObserver !== "function") window.addEventListener("scroll", check, { passive: true });
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
