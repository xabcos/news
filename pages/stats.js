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

  // One page view per page load; the news page changes only its query, which is not counted.
  const url = canonicalize(window.location.href);
  if (!url) return;
  const counter = document.querySelector("[data-page-views]");
  void fetch(trackUrl, {
    method: "POST",
    credentials: "omit",
    keepalive: true,
    referrerPolicy: "no-referrer",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url, referer: canonicalize(document.referrer) }),
  }).then(async (response) => {
    if (!counter || !response.ok) return;
    const { views } = await response.json();
    if (!counter.isConnected || !Number.isSafeInteger(views) || views < 1) return;
    counter.querySelector("[data-page-views-value]").textContent = new Intl.NumberFormat(document.documentElement.lang).format(views);
    counter.hidden = false;
  }).catch(() => {});
})();
