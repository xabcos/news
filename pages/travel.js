(() => {
  "use strict";

  // 𝑻𝒓𝒂𝒗𝒆𝒍 postcards, their viewer and the map: shared by the news page's tab and the
  // destination pages.
  const ROW = 4; // `grid-auto-rows` of the postcard grids, in pixels
  // The map's land runs from 84°N to 58°S (world.svg).
  const NORTH = 84;
  const SOUTH = -58;
  const TEXT = {
    zh: {
      more: (name) => `了解${name} →`, about: (name) => `关于${name}`, guide: "阅读导读 →", fact: "你知道吗",
      original: "原文", photos: (n) => `${n} 张照片`, photo: (i, n) => `第 ${i} / ${n} 张`,
      close: "关闭", previous: "上一张", next: "下一张", map: (n) => `最近 7 天 · ${n} 个目的地`,
    },
    en: {
      more: (name) => `About ${name} →`, about: (name) => `About ${name}`, guide: "Read the guide →", fact: "Did you know?",
      original: "Original", photos: (n) => `${n} photos`, photo: (i, n) => `Photo ${i} of ${n}`,
      close: "Close", previous: "Previous", next: "Next", map: (n) => `Last 7 days · ${n} ${n === 1 ? "destination" : "destinations"}`,
    },
  };
  const items = new WeakMap();
  let spans = null;
  let viewer = null;

  const english = () => document.documentElement.lang === "en";
  const text = () => TEXT[english() ? "en" : "zh"];
  const pick = (object, field) => (english() && object?.[`${field}_en`]) || object?.[field] || "";
  const destination = (slug) => `/travel/${encodeURIComponent(slug)}${english() ? "?lang=en" : ""}`;
  const photosOf = (item) => (item.travel?.photos?.length ? item.travel.photos : [item.image]).filter((photo) => photo?.url);
  const placeName = (place) => {
    const name = pick(place, "name");
    const country = pick(place, "country");
    return country && country !== name ? `${name} · ${country}` : name;
  };

  function element(tag, className, content = "") {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (content) node.textContent = content;
    return node;
  }

  function link(href, className, content, external = false) {
    const node = element("a", className, content);
    node.href = href;
    if (external) {
      node.target = "_blank";
      node.rel = "noopener noreferrer";
    }
    return node;
  }

  // Grid rows are ROW pixels tall and each card spans the rows its height needs, so cards of
  // any height pack into columns while the document keeps them in reading order. All heights
  // are read before any span is written: a write between reads would lay the page out again.
  function span(entries) {
    const spans = entries.map(({ target, borderBoxSize }) => {
      const height = borderBoxSize?.[0]?.blockSize ?? target.getBoundingClientRect().height;
      const gap = parseFloat(getComputedStyle(target).marginBottom) || 0;
      return [target, height ? String(Math.ceil((height + gap) / ROW)) : ""];
    });
    for (const [card, rows] of spans) {
      if (!rows || card.dataset.rows === rows) continue;
      card.dataset.rows = rows;
      card.style.gridRowEnd = `span ${rows}`;
    }
  }

  function postcard(item, { when = "" } = {}) {
    const photos = photosOf(item);
    const cover = photos[0];
    const card = element("article", "postcard");
    if (!cover) return card;
    const media = link(item.url, "postcard-media", "", true);
    const ratio = cover.width > 0 && cover.height > 0 ? cover.width / cover.height : 4 / 3;
    media.style.setProperty("--ratio", String(Math.min(2, Math.max(.6, ratio))));
    const image = element("img");
    image.src = cover.url;
    image.alt = pick(cover, "caption") || pick(item, "title");
    image.loading = "lazy";
    image.decoding = "async";
    image.referrerPolicy = "no-referrer";
    // A photograph the file server no longer keeps takes its card with it.
    image.addEventListener("error", () => card.remove(), { once: true });
    media.append(image);
    if (photos.length > 1) {
      const count = element("span", "postcard-count", String(photos.length));
      count.title = text().photos(photos.length);
      media.append(count);
    }
    // A plain click opens the viewer; modified clicks still open the post.
    media.addEventListener("click", (event) => {
      if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      event.preventDefault();
      open(card, 0);
    });
    const body = element("div", "postcard-body");
    const place = item.travel?.place;
    if (place?.slug) body.append(link(destination(place.slug), "postcard-place", placeName(place)));
    body.append(element("h3", "postcard-title", pick(item, "title")));
    const words = pick(item, "summary");
    if (words) body.append(element("p", "postcard-words", words));
    const meta = element("p", "postcard-meta");
    meta.append(link(item.url, "", item.source || "X", true));
    if (when) meta.append(` · ${when}`);
    body.append(meta);
    if (place?.slug) body.append(link(destination(place.slug), "postcard-more", text().more(pick(place, "name"))));
    card.append(media, body);
    items.set(card, item);
    if (typeof ResizeObserver === "function") {
      spans ||= new ResizeObserver(span);
      spans.observe(card);
    }
    return card;
  }

  function createViewer() {
    const dialog = element("dialog", "viewer");
    dialog.tabIndex = -1;
    const stage = element("div", "viewer-stage");
    const image = element("img", "viewer-image");
    image.decoding = "async";
    image.referrerPolicy = "no-referrer";
    const dots = element("div", "viewer-dots");
    stage.append(image, dots);
    const panel = element("aside", "viewer-panel");
    const button = (label, className, action) => {
      const node = element("button", `viewer-button ${className}`, label);
      node.type = "button";
      node.addEventListener("click", action);
      return node;
    };
    const close = button("×", "viewer-close", () => dialog.close());
    const previous = button("‹", "viewer-previous", () => step(-1));
    const next = button("›", "viewer-next", () => step(1));
    dialog.append(stage, panel, close, previous, next);
    dialog.addEventListener("close", () => document.body.classList.remove("dialog-open"));
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog || event.target === stage) dialog.close();
    });
    dialog.addEventListener("keydown", (event) => {
      if (event.key === "ArrowLeft") step(-1);
      else if (event.key === "ArrowRight") step(1);
    });
    // A horizontal swipe on the photograph turns the page on touch screens.
    let start = null;
    stage.addEventListener("pointerdown", (event) => {
      start = event.isPrimary ? { x: event.clientX, y: event.clientY } : null;
    });
    stage.addEventListener("pointercancel", () => { start = null; });
    stage.addEventListener("pointerup", (event) => {
      if (!start) return;
      const dx = event.clientX - start.x;
      const dy = event.clientY - start.y;
      start = null;
      if (Math.abs(dx) >= 48 && Math.abs(dx) > Math.abs(dy) * 1.2) step(dx < 0 ? 1 : -1);
    });
    document.body.append(dialog);
    return { dialog, image, dots, panel, close, previous, next, id: null, index: 0 };
  }

  function open(card, index) {
    viewer ||= createViewer();
    document.body.classList.add("dialog-open");
    if (!viewer.dialog.open) {
      viewer.dialog.showModal();
      viewer.dialog.focus();
    }
    show(card, index);
  }

  // The viewer walks every photograph of every postcard on the page, in order. It remembers
  // the moment by id because a background refresh draws new cards.
  const sequence = () => [...document.querySelectorAll(".postcard")].filter((card) => items.has(card));

  function step(offset) {
    const cards = sequence();
    const at = cards.findIndex((card) => items.get(card).id === viewer.id);
    if (at < 0) return;
    const index = viewer.index + offset;
    if (index >= 0 && index < photosOf(items.get(cards[at])).length) {
      show(cards[at], index);
      return;
    }
    const target = cards[at + offset];
    if (target) show(target, offset > 0 ? 0 : photosOf(items.get(target)).length - 1);
  }

  function show(card, index) {
    const item = items.get(card);
    if (!item) return;
    const photos = photosOf(item);
    const photo = photos[index];
    if (!photo) return;
    if (viewer.id !== item.id) panel(item);
    viewer.id = item.id;
    viewer.index = index;
    viewer.image.src = photo.url;
    viewer.image.alt = pick(photo, "caption") || pick(item, "title");
    viewer.dots.replaceChildren(...(photos.length > 1 ? photos.map((_, position) => {
      const dot = element("button", "viewer-dot");
      dot.type = "button";
      dot.setAttribute("aria-label", text().photo(position + 1, photos.length));
      if (position === index) dot.setAttribute("aria-current", "true");
      dot.addEventListener("click", () => show(card, position));
      return dot;
    }) : []));
    const cards = sequence();
    const at = cards.indexOf(card);
    viewer.previous.disabled = at <= 0 && index === 0;
    viewer.next.disabled = at < 0 || (at === cards.length - 1 && index === photos.length - 1);
    for (const [control, label] of [[viewer.close, text().close], [viewer.previous, text().previous], [viewer.next, text().next]]) {
      control.setAttribute("aria-label", label);
      control.title = label;
    }
  }

  function panel(item) {
    const place = item.travel?.place;
    const nodes = [];
    if (place?.slug) nodes.push(link(destination(place.slug), "viewer-place", placeName(place)));
    const spot = pick(item.travel, "spot");
    if (spot && spot !== pick(place, "name")) nodes.push(element("p", "viewer-spot", spot));
    nodes.push(element("h2", "viewer-title", pick(item, "title")));
    const words = pick(item, "summary");
    if (words) nodes.push(element("p", "viewer-words", words));
    const original = item.travel?.text;
    if (original && original !== words) {
      const details = element("details", "viewer-original");
      details.append(element("summary", "", text().original));
      const quote = element("p", "", original);
      if (item.travel.lang) quote.lang = item.travel.lang;
      details.append(quote);
      nodes.push(details);
    }
    nodes.push(link(item.url, "viewer-author", `${item.source || "X"} ↗`, true));
    if (place?.slug) {
      const about = element("section", "viewer-about");
      const tagline = pick(place, "tagline");
      if (tagline) {
        about.append(element("h3", "", text().about(pick(place, "name"))), element("p", "", tagline));
        const fact = pick(place, "fact");
        if (fact) {
          const line = element("p", "viewer-fact");
          line.append(element("strong", "", text().fact), fact);
          about.append(line);
        }
        about.append(link(destination(place.slug), "viewer-guide", text().guide));
      } else {
        about.append(link(destination(place.slug), "viewer-guide", text().more(pick(place, "name"))));
      }
      nodes.push(about);
    }
    viewer.panel.replaceChildren(...nodes);
    viewer.panel.scrollTop = 0;
  }

  // Destinations with recent moments on a dotted world map; a pin opens its page.
  async function map(container) {
    let places = [];
    try {
      const response = await fetch("/travel/map.json", { headers: { Accept: "application/json" } });
      if (response.ok) places = (await response.json()).places || [];
    } catch {
      // The tab works without its map.
    }
    container.replaceChildren();
    container.hidden = !places.length;
    if (!places.length) return;
    // Pins are placed on the canvas that holds the land, not on the figure with its caption.
    const figure = element("figure", "travel-map");
    const canvas = element("div", "travel-map-canvas");
    const land = element("div", "travel-map-land");
    land.setAttribute("aria-hidden", "true");
    canvas.append(land);
    figure.append(canvas);
    places.forEach((place, index) => {
      const pin = link(destination(place.slug), `travel-pin${index < 3 ? " is-recent" : ""}`);
      pin.setAttribute("aria-label", placeName(place));
      pin.style.left = `${((place.longitude + 180) / 360) * 100}%`;
      pin.style.top = `${((NORTH - Math.min(NORTH, Math.max(SOUTH, place.latitude))) / (NORTH - SOUTH)) * 100}%`;
      pin.append(element("span", "travel-pin-label", pick(place, "name")));
      canvas.append(pin);
    });
    figure.append(element("figcaption", "", text().map(places.length)));
    container.append(figure);
  }

  window.TravelUI = { postcard, map };

  // A destination page: its first moments arrive as JSON beside the server-rendered guide, and
  // scrolling to the end fetches the older ones a page at a time.
  function scroll(grid, format) {
    let next = grid.dataset.next;
    if (!next || typeof IntersectionObserver !== "function") return;
    const sentinel = element("div", "group-more");
    sentinel.setAttribute("aria-hidden", "true");
    grid.after(sentinel);
    let loading = false;
    const observer = new IntersectionObserver(async (entries) => {
      if (loading || !entries.some((entry) => entry.isIntersecting)) return;
      loading = true;
      try {
        const response = await fetch(`${grid.dataset.source}?before=${encodeURIComponent(next)}`, { headers: { Accept: "application/json" } });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const page = await response.json();
        grid.append(...page.moments.map((item) => postcard(item, { when: format.format(new Date(item.first_seen_at)) })));
        next = page.next;
      } catch {
        // The next intersection tries again.
      }
      loading = false;
      if (!next) {
        observer.disconnect();
        sentinel.remove();
      } else {
        // Re-observing reports the sentinel again if it is still in view.
        observer.unobserve(sentinel);
        observer.observe(sentinel);
      }
    }, { rootMargin: "0px 0px 70% 0px" });
    observer.observe(sentinel);
  }

  document.addEventListener("DOMContentLoaded", () => {
    const data = document.getElementById("moments");
    if (!document.querySelector(".destination")) return;
    PublicPage.init();
    try { localStorage.setItem("news.lang", english() ? "en" : "zh"); } catch { /* Reading works without storage. */ }
    document.querySelector('.tabs [aria-current="true"]')?.scrollIntoView({ block: "nearest", inline: "center" });
    const photo = document.querySelector(".destination-photo");
    const drop = () => {
      photo.closest(".destination-hero").classList.remove("has-photo");
      photo.remove();
    };
    if (photo?.complete && !photo.naturalWidth) drop();
    else photo?.addEventListener("error", drop, { once: true });
    const grid = document.querySelector("[data-moments]");
    if (!data || !grid) return;
    const format = new Intl.DateTimeFormat(english() ? "en" : "zh-CN", { month: "short", day: "numeric" });
    grid.append(...JSON.parse(data.textContent).map((item) => postcard(item, { when: format.format(new Date(item.first_seen_at)) })));
    scroll(grid, format);
  });
})();
