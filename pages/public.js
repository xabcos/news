(() => {
  "use strict";

  const preview = "/api/admin/preview";
  const DWELL = 12000; // milliseconds each photograph stays while a viewer plays, unless it says
  const role = () => document.cookie.split(";").map((part) => part.trim())
    .find((part) => part.startsWith("news_role="))?.slice("news_role=".length) || "";
  // Pages carry only public addresses; whatever their scripts read goes through here, so an
  // administrator reads the preview, which also holds what readers can't see yet.
  const readPath = (path) => role() === "admin" && !path.startsWith(preview) ? `${preview}${path}` : path;

  // Shared by the news feed and server-rendered public pages.
  window.PublicPage = {
    role,
    readPath,
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

    // Lets a photograph viewer play by itself. A photograph's time runs once it is on screen
    // in a visible page, drawn as a line along the top, and the screen stays awake meanwhile;
    // the one `ahead()` names loads in the background. `next()` shows the following
    // photograph and returns nothing at the end, which stops the play. The button, Space and
    // a tap on the photograph switch it, and closing stops it. The viewer calls `shown()`
    // whenever it changes the photograph. Each photograph stays `dwell` milliseconds.
    autoplay({ dialog, image, next, ahead, names, dwell = DWELL }) {
      const progress = document.createElement("div");
      progress.className = "play-progress";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "viewer-button play-button";
      dialog.prepend(progress, button);
      let playing = false;
      let walk = null;
      let lock = null;
      const advance = () => next() || toggle(false);
      const shown = () => {
        const name = names()[playing ? "pause" : "play"];
        button.setAttribute("aria-label", name);
        button.title = name;
        walk?.cancel();
        if (!playing || !image.complete || document.hidden) return;
        if (!image.naturalWidth) return advance(); // a photograph that failed is passed by
        walk = progress.animate({ transform: ["scaleX(0)", "scaleX(1)"] }, dwell);
        walk.onfinish = advance;
      };
      function toggle(on = !playing) {
        playing = on;
        dialog.classList.toggle("is-playing", on);
        // Each start takes a fresh wake lock; one still on its way is let go when it arrives.
        lock?.then((held) => held?.release());
        lock = on ? navigator.wakeLock?.request("screen").catch(() => null) : null;
        shown();
      }
      button.addEventListener("click", () => toggle());
      image.addEventListener("click", () => toggle());
      image.addEventListener("load", () => {
        image.animate({ opacity: [0, 1] }, 400);
        shown();
        const url = ahead();
        if (!url) return;
        const following = new Image();
        following.referrerPolicy = "no-referrer";
        following.src = url;
        following.decode().catch(() => {});
      });
      image.addEventListener("error", shown);
      dialog.addEventListener("keydown", (event) => {
        if (event.key !== " " || event.target !== dialog) return;
        event.preventDefault();
        toggle();
      });
      dialog.addEventListener("close", () => toggle(false));
      // The browser takes the wake lock back while the page is hidden; coming back takes a
      // new one and gives the photograph its time again.
      document.addEventListener("visibilitychange", () => playing && toggle(true));
      return { toggle, shown };
    },

    // A photograph viewer over the page: close, previous and next buttons, the arrow keys, a
    // swipe on `frame`, a click beside the photograph to close it, and play. `step(offset)`
    // shows the photograph that many places away and returns it, or nothing at either end;
    // `neighbor(offset)` only finds it. Cards that load into `root` behind the open viewer
    // bring its next arrow back.
    viewer({ className = "", frame, image, root, step, neighbor, ahead, names, dwell }) {
      const dialog = document.createElement("dialog");
      dialog.className = `viewer ${className}`.trim();
      dialog.tabIndex = -1;
      image.classList.add("viewer-image");
      image.decoding = "async";
      image.referrerPolicy = "no-referrer";
      const button = (label, kind, action) => {
        const node = document.createElement("button");
        node.type = "button";
        node.className = `viewer-button viewer-${kind}`;
        node.textContent = label;
        node.addEventListener("click", action);
        return node;
      };
      const close = button("×", "close", () => dialog.close());
      const previous = button("‹", "previous", () => step(-1));
      const next = button("›", "next", () => step(1));
      dialog.append(frame, close, previous, next);
      dialog.addEventListener("close", () => document.body.classList.remove("dialog-open"));
      dialog.addEventListener("click", (event) => {
        if (event.target === dialog || event.target === frame) dialog.close();
      });
      dialog.addEventListener("keydown", (event) => {
        if (event.key === "ArrowLeft") step(-1);
        else if (event.key === "ArrowRight") step(1);
      });
      new MutationObserver(() => { if (dialog.open) next.disabled = !neighbor(1); })
        .observe(root, { childList: true, subtree: true });
      const play = window.PublicPage.autoplay({
        dialog, image, next: () => step(1), ahead, names, dwell,
      });
      // A horizontal swipe on the photograph turns the page on touch screens.
      let start = null;
      frame.addEventListener("pointerdown", (event) => {
        start = event.isPrimary ? { x: event.clientX, y: event.clientY } : null;
      });
      frame.addEventListener("pointercancel", () => { start = null; });
      frame.addEventListener("pointerup", (event) => {
        if (!start) return;
        const dx = event.clientX - start.x;
        const dy = event.clientY - start.y;
        start = null;
        if (Math.abs(dx) >= 48 && Math.abs(dx) > Math.abs(dy) * 1.2) step(dx < 0 ? 1 : -1);
      });
      document.body.append(dialog);
      return {
        play,
        // Opens the viewer and tells whether it was closed until now.
        open() {
          document.body.classList.add("dialog-open");
          if (dialog.open) return false;
          dialog.showModal();
          // Focus the viewer itself: the arrow keys work at once and no button shows a ring.
          dialog.focus();
          return true;
        },
        // After a photograph is put in place: its time, the arrows and their names. The page
        // follows behind, so closing returns to `card` and endless scrolling keeps loading.
        shown(card) {
          play.shown();
          previous.disabled = !neighbor(-1);
          next.disabled = !neighbor(1);
          const label = names();
          for (const [control, name] of [[close, label.close], [previous, label.previous], [next, label.next]]) {
            control.setAttribute("aria-label", name);
            control.title = name;
          }
          card.scrollIntoView({ block: "nearest", behavior: "instant" });
        },
      };
    },

    // A server-rendered page: `init`, its language kept for the news page, and its tab in view.
    page() {
      window.PublicPage.init();
      const lang = document.documentElement.lang === "en" ? "en" : "zh";
      try { localStorage.setItem("news.lang", lang); } catch { /* Reading works without storage. */ }
      document.querySelector('.tabs [aria-current="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
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
        // A catalogue section's collections take the swipe first; past either end it moves on to
        // the neighbouring topic, and coming back from the right enters the last collection.
        const offset = dx < 0 ? 1 : -1;
        const current = (tabs) => tabs.findIndex((tab) => tab.getAttribute("aria-current") === "true");
        const collections = [...document.querySelectorAll(".catalog-tabs a")];
        let target = collections[current(collections) + offset];
        if (!target) {
          const tabs = [...topbar.querySelectorAll(".tab")];
          const at = current(tabs);
          if (at < 0 || tabs.length < 2) return;
          target = tabs[(at + offset + tabs.length) % tabs.length];
        }
        topbar.classList.remove("is-hidden");
        document.scrollingElement.scrollTop = 0;
        if (offset < 0 && target.dataset.last) location.assign(target.dataset.last);
        else target.click();
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
