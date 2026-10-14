"use strict";

PublicPage.page();

// A read counts once a day in this browser, and the server leaves out the author's own. The
// page may come from a cache, so the count it shows is brought up to date here.
(() => {
  const counter = document.querySelector("[data-article-views]");
  const id = counter?.dataset.articleViews;
  if (!id) return;
  const key = `news.read.${id}`;
  const today = new Date().toDateString();
  try {
    if (localStorage.getItem(key) === today) return;
  } catch { /* Counting works without storage. */ }
  fetch(`/api/articles/${encodeURIComponent(id)}/views`, { method: "POST", credentials: "same-origin" })
    .then((response) => (response.ok ? response.json() : null))
    .then((data) => {
      if (!Number.isSafeInteger(data?.views)) return;
      try { localStorage.setItem(key, today); } catch { /* Counting works without storage. */ }
      counter.querySelector("span").textContent = new Intl.NumberFormat(document.documentElement.lang).format(data.views);
      counter.hidden = data.views < 1;
    })
    .catch(() => {});
})();

// An administrator's preview: the ❤️ publishes the revision on screen, and taking it back
// returns the article to review.
(() => {
  const control = document.querySelector("[data-article-review]");
  if (!control) return;
  control.addEventListener("click", async () => {
    control.disabled = true;
    try {
      const response = await fetch(`/api/admin/articles/${encodeURIComponent(control.dataset.articleReview)}`, {
        method: "PATCH",
        credentials: "same-origin",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ published: control.dataset.published !== "true", revision: Number(control.dataset.revision) }),
      });
      if (!response.ok) {
        const data = await response.json().catch(() => null);
        throw new Error(data?.error?.message || `HTTP ${response.status}`);
      }
      location.reload();
    } catch (error) {
      control.disabled = false;
      window.alert(error.message);
    }
  });
})();
