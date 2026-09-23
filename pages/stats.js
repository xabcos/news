(() => {
  "use strict";

  if (
    !["http:", "https:"].includes(window.location.protocol)
    || ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].includes(window.location.hostname)
  ) return;
  if (window.__statsTrackerInstalled) return;

  const trackUrl = (() => {
    try {
      const value = document.currentScript?.dataset.trackUrl || "";
      const url = new URL(value, window.location.href);
      return url.protocol === "https:" && !url.username && !url.password ? url.href : "";
    } catch {
      return "";
    }
  })();
  if (!trackUrl) return;
  window.__statsTrackerInstalled = true;

  const canonicalize = (value) => {
    if (!value) return "";
    try {
      const url = new URL(value, window.location.href);
      url.search = "";
      url.hash = "";
      return url.href;
    } catch {
      return "";
    }
  };

  const referrer = canonicalize(document.referrer);
  let currentUrl = "";
  let generation = 0;

  const track = () => {
    const url = canonicalize(window.location.href);
    if (!url || url === currentUrl) return;

    const referer = currentUrl || referrer;
    currentUrl = url;
    const request = ++generation;
    const counter = document.querySelector("[data-page-views]");
    if (counter) counter.hidden = true;
    void fetch(trackUrl, {
      method: "POST",
      credentials: "omit",
      keepalive: true,
      referrerPolicy: "no-referrer",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url, referer }),
    }).then(async (response) => {
      if (!counter || !response.ok) return;
      const { views } = await response.json();
      if (request !== generation || !counter.isConnected || !Number.isSafeInteger(views) || views < 1) return;
      counter.querySelector("[data-page-views-value]").textContent = new Intl.NumberFormat(document.documentElement.lang).format(views);
      counter.hidden = false;
    }).catch(() => {});
  };

  ["pushState", "replaceState"].forEach((method) => {
    const original = history[method];
    if (typeof original !== "function") return;
    history[method] = function (...args) {
      const result = original.apply(this, args);
      track();
      return result;
    };
  });

  window.addEventListener("popstate", track);
  track();
})();
