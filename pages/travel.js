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
      more: (name) => `了解${name} →`, about: (name) => `关于${name}`, guide: "阅读导读 →",
      original: "原文", photos: (n) => `${n} 张照片`, photo: (i, n) => `第 ${i} / ${n} 张`,
      close: "关闭", previous: "上一张", next: "下一张", map: (n, recent) => `${n} 个目的地${recent < n ? ` · 最近 7 天 ${recent} 个` : ""}`,
      wander: "漫游", pause: "暂停", favorite: "最近 7 天最受喜爱",
      sections: { highlights: "必看", flavors: "风味", people: "人物", culture: "文化", season: "时节与节庆", customs: "入乡随俗", origin: "地名由来", history: "历史" },
    },
    en: {
      more: (name) => `About ${name} →`, about: (name) => `About ${name}`, guide: "Read the guide →",
      original: "Original", photos: (n) => `${n} photos`, photo: (i, n) => `Photo ${i} of ${n}`,
      close: "Close", previous: "Previous", next: "Next", map: (n, recent) => `${n} ${n === 1 ? "destination" : "destinations"}${recent < n ? ` · ${recent} in the last 7 days` : ""}`,
      wander: "Wander", pause: "Pause", favorite: "Most liked in the last 7 days",
      sections: { highlights: "Must-see", flavors: "Flavors", people: "People", culture: "Culture", season: "Seasons & festivals", customs: "Local customs", origin: "The name", history: "History" },
    },
  };
  const items = new WeakMap();
  let spans = null;
  let viewer = null;

  const english = () => document.documentElement.lang === "en";
  const text = () => TEXT[english() ? "en" : "zh"];
  const pick = (object, field) => (english() && object?.[`${field}_en`]) || object?.[field] || "";
  const destination = (slug) => PublicPage.readPath(`/travel/${encodeURIComponent(slug)}${english() ? "?lang=en" : ""}`);
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

  // The dotted land that pins are placed on.
  function land() {
    const canvas = element("div", "travel-map-canvas");
    const dots = element("div", "travel-map-land");
    dots.setAttribute("aria-hidden", "true");
    canvas.append(dots);
    return canvas;
  }

  function locate(node, { latitude, longitude }) {
    node.style.left = `${((longitude + 180) / 360) * 100}%`;
    node.style.top = `${((NORTH - Math.min(NORTH, Math.max(SOUTH, latitude))) / (NORTH - SOUTH)) * 100}%`;
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
    // A photograph the file server no longer keeps hides its card; the viewer steps over it.
    image.addEventListener("error", () => { card.hidden = true; }, { once: true });
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
    spans ||= new ResizeObserver(span);
    spans.observe(card);
    return card;
  }

  function createViewer() {
    const stage = element("div", "viewer-stage");
    const image = element("img");
    const dots = element("div", "viewer-dots");
    stage.append(image, dots);
    // The map stays while the story beside it changes, so its pin glides from place to
    // place; faint dots stay on the places already passed.
    const map = land();
    map.classList.add("viewer-map");
    const pin = element("span", "travel-pin is-recent");
    map.append(pin);
    const story = element("div", "viewer-story");
    const panel = element("aside", "viewer-panel");
    panel.append(map, story);
    // Wandering walks on through every photograph of every postcard.
    const ui = PublicPage.viewer({
      frame: stage,
      image,
      root: document.querySelector("#digest, [data-moments]"),
      step,
      neighbor,
      ahead: () => {
        const target = neighbor(1);
        return target && photosOf(items.get(target[0]))[target[1]].url;
      },
      names: () => ({
        play: text().wander, pause: text().pause,
        close: text().close, previous: text().previous, next: text().next,
      }),
    });
    stage.after(panel);
    return { ui, image, dots, panel, map, pin, story, id: null, index: 0, passed: new Set() };
  }

  function open(card, index) {
    viewer ||= createViewer();
    // The page may have switched language since the viewer was last open.
    if (viewer.ui.open()) viewer.id = null;
    show(card, index);
  }

  // The viewer walks every photograph of every postcard on the page, in order. It remembers
  // the moment by id because a background refresh draws new cards.
  const sequence = () => [...document.querySelectorAll(".postcard")].filter((card) => items.has(card));

  // The card and photograph `offset` steps from the one shown, or null at either end.
  function neighbor(offset) {
    const cards = sequence();
    let at = cards.findIndex((card) => items.get(card).id === viewer.id);
    if (at < 0) return null;
    const index = viewer.index + offset;
    if (index >= 0 && index < photosOf(items.get(cards[at])).length) return [cards[at], index];
    do at += offset; while (cards[at]?.hidden);
    const card = cards[at];
    return card ? [card, offset > 0 ? 0 : photosOf(items.get(card)).length - 1] : null;
  }

  function step(offset) {
    const target = neighbor(offset);
    if (target) show(...target);
    return target;
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
    viewer.ui.shown(card);
  }

  // Each destination's guide, fetched once when the viewer first shows one of its moments. A
  // destination without a guide answers 404 and stays null; a failed request is tried again.
  const guides = new Map();
  function guideOf(slug) {
    if (!guides.has(slug)) {
      guides.set(slug, fetch(PublicPage.readPath(`/travel/${encodeURIComponent(slug)}/guide.json`), { headers: { Accept: "application/json" } })
        .then(async (response) => (response.ok ? (await response.json()).guide : response.status === 404 ? null : Promise.reject()))
        .catch(() => {
          guides.delete(slug);
          return null;
        }));
    }
    return guides.get(slug);
  }

  const hash = (value) => [...String(value)].reduce((sum, char) => (sum * 31 + char.charCodeAt(0)) >>> 0, 0);

  // One part of the guide beside a photograph: the sight, dish or person the photograph's spot
  // names, otherwise a part chosen by the moment, so a moment keeps its part and neighbors differ.
  function guidePart(guide, item) {
    const label = text().sections;
    const parts = [];
    for (const key of ["highlights", "flavors", "people"]) {
      for (const entry of guide[key] || []) {
        const words = pick(entry, "text");
        if (words) parts.push({ label: label[key], name: pick(entry, "name"), text: words, names: [entry.name, entry.name_en] });
      }
    }
    for (const key of ["culture", "season", "customs", "origin"]) {
      const words = pick(guide, key);
      if (words) parts.push({ label: label[key], text: words });
    }
    for (const paragraph of pick(guide, "history").split("\n").map((line) => line.trim()).filter(Boolean)) {
      parts.push({ label: label.history, text: paragraph });
    }
    if (!parts.length) return null;
    // A spot the destination's own name contains, such as 京都 in 京都市, names nothing in
    // particular; one that contains it, such as 伦敦塔 in 伦敦, is a sight.
    const place = [item.travel?.place?.name, item.travel?.place?.name_en].filter(Boolean).map((name) => name.toLowerCase());
    const spots = [item.travel?.spot, item.travel?.spot_en].filter(Boolean).map((spot) => spot.toLowerCase())
      .filter((spot) => !place.some((name) => name.includes(spot)));
    const named = parts.find((part) => part.names?.some((name) => {
      const known = name?.toLowerCase();
      return known?.length > 1 && spots.some((spot) => spot.includes(known) || known.includes(spot));
    }));
    return named || parts[hash(item.id) % parts.length];
  }

  function panel(item) {
    const place = item.travel?.place;
    viewer.map.hidden = place?.latitude == null;
    if (!viewer.map.hidden) {
      locate(viewer.pin, place);
      if (!viewer.passed.has(place.slug)) {
        viewer.passed.add(place.slug);
        viewer.pin.before(locate(element("i", "viewer-trail"), place));
      }
    }
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
      details.addEventListener("toggle", () => details.open && viewer.ui.play.toggle(false));
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
        if (fact) about.append(element("p", "viewer-fact", fact));
        const more = link(destination(place.slug), "viewer-guide", text().guide);
        about.append(more);
        // The part of the guide arrives after the panel; the viewer may have moved on by then.
        guideOf(place.slug).then((guide) => {
          const part = guide && viewer.id === item.id && guidePart(guide, item);
          if (!part) return;
          const node = element("div", "viewer-part");
          node.append(element("p", "viewer-part-label", part.name ? `${part.label} · ${part.name}` : part.label), element("p", "", part.text));
          more.before(node);
        });
      } else {
        about.append(link(destination(place.slug), "viewer-guide", text().more(pick(place, "name"))));
      }
      nodes.push(about);
    }
    viewer.story.replaceChildren(...nodes);
    viewer.panel.scrollTop = 0;
  }

  // One of the week's most liked moments beside the map; it leads to its destination.
  function favorite(moment, side) {
    const card = link(destination(moment.slug), `travel-favorite is-${side}`);
    card.title = text().favorite;
    const image = element("img");
    image.src = moment.photo.url;
    image.alt = pick(moment, "title");
    image.decoding = "async";
    image.referrerPolicy = "no-referrer";
    image.addEventListener("error", () => { card.hidden = true; }, { once: true });
    const caption = element("span", "travel-favorite-text");
    caption.append(element("span", "travel-favorite-place", placeName(moment)), element("strong", "", pick(moment, "title")));
    if (moment.likes > 0) {
      const count = new Intl.NumberFormat(english() ? "en" : "zh-CN", { notation: "compact" }).format(moment.likes);
      caption.append(element("span", "travel-favorite-likes", `\u2665\uFE0E ${count}`));
    }
    card.append(image, caption);
    return card;
  }

  // Two of the offered favorites, picked anew on each visit.
  function pickTwo(moments) {
    const pool = moments.filter((moment) => moment.photo?.url);
    for (let index = pool.length - 1; index > 0; index -= 1) {
      const other = Math.floor(Math.random() * (index + 1));
      [pool[index], pool[other]] = [pool[other], pool[index]];
    }
    return pool.slice(0, 2);
  }

  // Every destination on a dotted world map, the recent ones larger; a pin opens its page. Wide
  // screens show two of the week's most liked moments on either side.
  async function map(container) {
    let places = [];
    let featured = [];
    try {
      const response = await fetch(PublicPage.readPath("/travel/map.json"), { headers: { Accept: "application/json" } });
      if (response.ok) ({ places = [], featured = [] } = await response.json());
    } catch {
      // The tab works without its map.
    }
    container.replaceChildren();
    container.hidden = !places.length;
    if (!places.length) return;
    // Pins are placed on the canvas that holds the land, not on the figure with its caption.
    const figure = element("figure", "travel-map");
    const canvas = land();
    figure.append(canvas);
    places.forEach((place, index) => {
      const pin = link(destination(place.slug), `travel-pin${place.recent === false ? " is-past" : index < 3 ? " is-recent" : ""}`);
      pin.setAttribute("aria-label", placeName(place));
      pin.append(element("span", "travel-pin-label", pick(place, "name")));
      canvas.append(locate(pin, place));
    });
    // Wandering starts from the newest postcard.
    const start = element("button", "travel-wander", `\u25B6\uFE0E ${text().wander}`);
    start.type = "button";
    start.addEventListener("click", () => {
      const [first] = sequence();
      if (!first) return;
      open(first, 0);
      viewer.ui.play.toggle(true);
    });
    const caption = element("figcaption", "", text().map(places.length, places.filter((place) => place.recent !== false).length));
    caption.append(start);
    figure.append(caption);
    const atlas = element("div", "travel-atlas");
    atlas.append(figure);
    container.append(atlas);
    // The atlas is a grid only on a wide screen with this version's stylesheet, so a narrow
    // screen downloads no favorites, and neither does a page whose stylesheet is older.
    const sides = pickTwo(featured);
    if (sides.length === 2 && getComputedStyle(atlas).display === "grid") {
      figure.before(favorite(sides[0], "left"));
      figure.after(favorite(sides[1], "right"));
    }
  }

  window.TravelUI = { postcard, map };

  // A destination page: its first moments arrive as JSON beside the server-rendered guide, and
  // scrolling to the end fetches the older ones a page at a time.
  function scroll(grid, format) {
    let next = grid.dataset.next;
    if (!next) return;
    const sentinel = element("div", "group-more");
    sentinel.setAttribute("aria-hidden", "true");
    grid.after(sentinel);
    let loading = false;
    const observer = new IntersectionObserver(async (entries) => {
      if (loading || !entries.some((entry) => entry.isIntersecting)) return;
      loading = true;
      try {
        // The source may already carry a filter.
        const url = new URL(grid.dataset.source, location.href);
        url.searchParams.set("before", next);
        const response = await fetch(url, { headers: { Accept: "application/json" } });
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
    PublicPage.page();
    const photo = document.querySelector(".destination-photo");
    const drop = () => {
      photo.closest(".destination-hero").classList.remove("has-photo");
      photo.remove();
    };
    if (photo?.complete && !photo.naturalWidth) drop();
    else photo?.addEventListener("error", drop, { once: true });
    // A nearby destination whose photograph the file server no longer keeps steps aside.
    for (const image of document.querySelectorAll(".nearby-card img")) {
      const hide = () => { image.closest(".nearby-card").hidden = true; };
      if (image.complete && !image.naturalWidth) hide();
      else image.addEventListener("error", hide, { once: true });
    }
    const grid = document.querySelector("[data-moments]");
    if (!data || !grid) return;
    const format = new Intl.DateTimeFormat(english() ? "en" : "zh-CN", { month: "short", day: "numeric" });
    grid.append(...JSON.parse(data.textContent).map((item) => postcard(item, { when: format.format(new Date(item.first_seen_at)) })));
    scroll(grid, format);
  });
})();
