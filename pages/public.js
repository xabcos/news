(() => {
  "use strict";

  // Shared by the news feed and the server-rendered Reads pages.
  window.PublicPage = {
    // An external link opened in a new tab, or null for anything but plain http(s).
    link(url, label, title = "") {
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
    },

    init({ canSwipe = () => true } = {}) {
      const topbar = document.querySelector(".topbar");
      const backToTop = document.querySelector("#backToTop");
      let last = window.scrollY;
      let visible = false;
      let anchor = last;
      const update = () => {
        const y = Math.max(0, window.scrollY);
        if (y <= 48 || Math.abs(y - last) > 4) topbar.classList.toggle("is-hidden", y > last && y > 48);
        anchor = visible ? Math.min(anchor, y) : Math.max(anchor, y);
        if (y <= 100) {
          visible = false;
          anchor = y;
        } else if (Math.abs(y - anchor) >= 8) {
          visible = y < anchor && y > window.innerHeight;
          anchor = y;
        }
        backToTop.classList.toggle("is-visible", visible);
        last = y;
      };
      window.addEventListener("scroll", update, { passive: true });
      window.addEventListener("resize", update);
      window.addEventListener("pageshow", update);
      backToTop.addEventListener("click", () => {
        topbar.classList.remove("is-hidden");
        topbar.querySelector(".brand").focus({ preventScroll: true });
        window.scrollTo({ top: 0, behavior: "auto" });
      });
      update();

      if (typeof window.PointerEvent !== "function") return;
      const mobile = window.matchMedia("(max-width: 768px)");
      let gesture = null;
      let suppressClickUntil = 0;
      document.addEventListener("pointerdown", (event) => {
        if (event.isPrimary) suppressClickUntil = 0;
        else gesture = null;
      }, true);
      document.querySelector(".shell").addEventListener("pointerdown", (event) => {
        gesture = null;
        if (!event.isPrimary || event.pointerType !== "touch" || !mobile.matches
          || event.target.closest("button, input, textarea, select, [contenteditable], pre, table")
          || !window.getSelection().isCollapsed || !canSwipe(event.target)) return;
        // Leave any horizontally scrollable content to the browser.
        for (let node = event.target; node && !node.matches(".shell"); node = node.parentElement) {
          if (node.scrollWidth > node.clientWidth + 1 && /auto|scroll/.test(getComputedStyle(node).overflowX)) return;
        }
        gesture = { id: event.pointerId, x: event.clientX, y: event.clientY };
      }, { passive: true });
      window.addEventListener("pointerup", (event) => {
        if (!gesture || event.pointerId !== gesture.id) return;
        const { x, y } = gesture;
        gesture = null;
        const dx = event.clientX - x;
        const dy = event.clientY - y;
        if (Math.abs(dx) < 48 || Math.abs(dx) <= Math.abs(dy) * 1.2 || !window.getSelection().isCollapsed) return;
        const tabs = [...topbar.querySelectorAll(".tabs .tab")];
        const current = tabs.findIndex((tab) => tab.getAttribute("aria-current") === "true");
        if (current < 0 || tabs.length < 2) return;
        const next = (current + (dx < 0 ? 1 : -1) + tabs.length) % tabs.length;
        topbar.classList.remove("is-hidden");
        document.scrollingElement.scrollTop = 0;
        tabs[next].click();
        suppressClickUntil = performance.now() + 500;
      }, { passive: true });
      window.addEventListener("pointercancel", () => { gesture = null; }, { passive: true });
      document.addEventListener("click", (event) => {
        if (!suppressClickUntil || performance.now() > suppressClickUntil || event.detail === 0) return;
        suppressClickUntil = 0;
        event.preventDefault();
        event.stopPropagation();
      }, true);
    },
  };
})();
