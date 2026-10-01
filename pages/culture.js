(() => {
  "use strict";
  const en = document.documentElement.lang === "en";
  const labels = en ? { more: "More stories", loading: "Loading…", retry: "Try again" } : { more: "更多动态", loading: "正在加载…", retry: "重试" };
  PublicPage.init();
  try { localStorage.setItem("news.lang", en ? "en" : "zh"); } catch {}
  document.querySelector('.tabs [aria-current="true"]')?.scrollIntoView({ block: "nearest", inline: "center" });
  document.addEventListener("error", (event) => {
    if (event.target instanceof HTMLImageElement) (event.target.closest(".image-news-media") || event.target).remove();
  }, true);
  // Also catch images that failed before this deferred script ran.
  document.querySelectorAll(".shell img").forEach((img) => {
    if (img.complete && !img.naturalWidth) (img.closest(".image-news-media") || img).remove();
  });
  const more = document.querySelector(".culture-more");
  if (!more) return;
  let loading = false;
  async function load(event) {
    event?.preventDefault();
    if (loading) return;
    loading = true;
    more.setAttribute("aria-busy", "true");
    more.textContent = labels.loading;
    try {
      const url = new URL(more.dataset.source, location.origin);
      url.searchParams.set("before", more.dataset.before);
      if (en) url.searchParams.set("lang", "en");
      const response = await fetch(url, { signal: AbortSignal.timeout(20000) });
      if (!response.ok) throw new Error(String(response.status));
      const page = await response.json();
      document.querySelector(".culture-grid").insertAdjacentHTML("beforeend", page.html);
      if (!page.next) { more.remove(); observer?.disconnect(); return; }
      more.dataset.before = page.next;
      const href = new URL(more.href);
      href.searchParams.set("before", page.next);
      more.href = href.href;
      more.textContent = labels.more;
    } catch {
      more.textContent = labels.retry;
      observer?.unobserve(more);
    } finally {
      loading = false;
      more.removeAttribute("aria-busy");
    }
  }
  more.addEventListener("click", load);
  const observer = typeof IntersectionObserver === "function" ? new IntersectionObserver((entries) => {
    if (entries.some((entry) => entry.isIntersecting)) load();
  }, { rootMargin: "300px" }) : null;
  observer?.observe(more);
})();
