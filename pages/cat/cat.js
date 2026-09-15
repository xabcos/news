"use strict";

/*
 * Travel Cat's first page deliberately keeps three boundaries visible:
 *
 * - CatScene owns only presentation and local interaction feedback.
 * - CatShell wires the two together. Future account state, trip jobs and image
 *   generation can replace one boundary without changing the other two.
 */

const REALISTIC_CATS = Object.freeze({
  orange_white: "/cat/assets/character/cat-realistic-orange-white.webp",
  cream_tabby: "/cat/assets/character/cat-realistic-cream-tabby.webp",
  silver_tabby: "/cat/assets/character/cat-realistic-silver-tabby.webp",
  black_white: "/cat/assets/character/cat-realistic-black-white.webp",
});

// The stage and the "此刻" heading describe the same presence, so both read from here.
const PRESENCE_LABELS = Object.freeze({
  home_awake: "猫咪正在窗边休息",
  home_sleeping: "猫咪已经睡着了",
  away: "猫咪正在旅途中",
  returning_soon: "猫咪快要回来了",
  vigil: "猫咪在灯下安静地休息",
});
const PRESENCE_CAPTIONS = Object.freeze({
  home_awake: "在家 · 安静地生活",
  home_sleeping: "睡着了 · 房间替它守着夜",
  away: "在路上 · 正在收集新的照片",
  returning_soon: "快回来了 · 门口的灯已经亮起",
  vigil: "今晚在家 · 点一盏灯陪自己安静下来",
});
const PRESENCE_SHORT = Object.freeze({
  home_awake: "在家", home_sleeping: "睡着了", away: "在路上", returning_soon: "快回来了", vigil: "今晚在家",
});
// "travel" is the default stream of the whole page; labelling every card with it only
// repeats the panel heading, so it is deliberately absent here.
const MESSAGE_LABELS = Object.freeze({
  welcome: "第一次相遇", postcard: "旅程照片", growth: "成长记录", encounter: "共同世界",
});
// Titles the backend stores that say nothing the card does not already say.
const REDUNDANT_TITLES = new Set(["猫咪动态", "猫咪记录", "猫咪的一条记录"]);

const CatScene = (() => {
  const elements = {
    stage: document.querySelector(".cat-stage"),
    companion: document.querySelector("#catCompanion"),
    realisticImage: document.querySelector("#catRealisticImage"),
    latestPhoto: document.querySelector("#catLatestPhoto"),
    thought: document.querySelector("#catThought"),
    caption: document.querySelector("#catSceneCaption"),
    identityMeta: document.querySelector("#catIdentityMeta"),
    toast: document.querySelector("#catToast"),
  };
  let toastTimer;
  let idleTimer;
  let idleActionTimer;
  let lastPresence;
  let lastPresenceNote = "";
  let lastSceneSignature;
  let lastIdentityKey;
  let lastPhotoUrl = "";
  let displayedPhotoUrl = "";
  let photoLoadToken = 0;
  let photoSwapTimer;
  const IDLE_ACTIONS = ["blink", "look", "tail", "stretch"];
  const ACTIVITY_GLYPHS = Object.freeze({
    observe: "⌁", packing: "⌂", reading: "✉", wandering: "↝", stretching: "⌒",
    sleeping: "☾", daydreaming: "·",
  });
  const PROP_GLYPHS = Object.freeze({ scarf: "▰", satchel: "▱", suitcase: "▣", letter: "✉", telescope: "◌" });

  const sceneOverlay = document.createElement("span");
  sceneOverlay.className = "cat-scene-overlay";
  sceneOverlay.setAttribute("aria-hidden", "true");
  const motionOverlay = document.createElement("span");
  motionOverlay.className = "cat-scene-motion";
  motionOverlay.setAttribute("aria-hidden", "true");
  elements.companion?.append(sceneOverlay, motionOverlay);

  function clearRealisticArt() {
    elements.companion.classList.remove("has-realistic-art");
    elements.companion.classList.remove("is-identity-pending");
    elements.realisticImage?.removeAttribute("src");
  }

  // Bind this before any account state is loaded so the anonymous preview
  // also falls back to the deterministic SVG when the raster cannot load.
  if (elements.realisticImage) elements.realisticImage.onerror = clearRealisticArt;

  function showToast(message) {
    window.clearTimeout(toastTimer);
    elements.toast.textContent = message;
    elements.toast.classList.add("is-visible");
    toastTimer = window.setTimeout(() => elements.toast.classList.remove("is-visible"), 2600);
  }

  function react(message, thought = message) {
    elements.thought.textContent = thought;
    elements.companion.classList.remove("is-reacting");
    void elements.companion.offsetWidth;
    elements.companion.classList.add("is-reacting");
    showToast(message);
  }

  function setIdleAction(action) {
    if (!elements.companion) return;
    IDLE_ACTIONS.forEach((name) => elements.companion.classList.remove(`is-idle-${name}`));
    if (!action || elements.companion.dataset.presence === "away") return;
    elements.companion.classList.add(`is-idle-${action}`);
    window.clearTimeout(idleActionTimer);
    idleActionTimer = window.setTimeout(() => {
      elements.companion.classList.remove(`is-idle-${action}`);
    }, action === "stretch" ? 1500 : 850);
  }

  function scheduleIdle() {
    window.clearTimeout(idleTimer);
    idleTimer = window.setTimeout(() => {
      if (document.visibilityState === "visible") {
        const presence = elements.companion.dataset.presence;
        if (presence !== "home_sleeping" && presence !== "away") {
          setIdleAction(IDLE_ACTIONS[Math.floor(Math.random() * IDLE_ACTIONS.length)]);
        }
      }
      scheduleIdle();
    }, 5200 + Math.random() * 7200);
  }

  function normalizeScene(scene, trip) {
    const narrative = trip?.souvenir?.narrative || trip?.narrative;
    const source = scene && typeof scene === "object" ? scene : {};
    const value = (key) => typeof source[key] === "string" ? source[key].trim() : "";
    return {
      activity: value("activity") || value("action") || value("pose") || value("animation"),
      mood: value("mood") || value("emotion"),
      prop: value("prop") || value("object") || value("accessory"),
      thought: value("thought") || value("bubble") || value("bubble_zh") || value("scene_hint") || narrative?.scene_hint || "",
      caption: value("caption") || value("caption_zh") || value("description"),
      weather: value("weather"),
    };
  }

  function setScene(scene, presence, trip) {
    const normalized = normalizeScene(scene, trip);
    const sceneSignature = JSON.stringify(normalized);
    if (sceneSignature === lastSceneSignature) return;
    lastSceneSignature = sceneSignature;
    const canonical = (value, aliases) => {
      const raw = value.toLowerCase().replaceAll(" ", "_");
      return aliases[raw] || raw;
    };
    const activity = canonical(normalized.activity, {
      "观察": "observe", "观察远方": "observe", "看风景": "observe", "整理行囊": "packing",
      "收拾行李": "packing", "读信": "reading", "阅读": "reading", "散步": "wandering",
      "伸懒腰": "stretching", "睡觉": "sleeping", "打盹": "sleeping", "发呆": "daydreaming",
    });
    const mood = canonical(normalized.mood, {
      "平静": "calm", "好奇": "curious", "开心": "happy", "雀跃": "happy", "困倦": "sleepy",
      "思念": "wistful", "惊讶": "surprised", "专注": "focused", "调皮": "playful",
    });
    const prop = canonical(normalized.prop, { "围巾": "scarf", "挎包": "satchel", "行李箱": "suitcase", "信": "letter", "望远镜": "telescope" });
    ["activity", "mood", "prop", "weather"].forEach((key) => {
      const value = key === "activity" ? activity : key === "mood" ? mood : key === "prop" ? prop : normalized.weather;
      if (value) {
        elements.stage.dataset[key] = value;
        elements.companion.dataset[key] = value;
      } else {
        delete elements.stage.dataset[key];
        delete elements.companion.dataset[key];
      }
    });
    sceneOverlay.textContent = PROP_GLYPHS[prop] || "";
    motionOverlay.textContent = ACTIVITY_GLYPHS[activity] || "";
    sceneOverlay.hidden = !PROP_GLYPHS[prop];
    motionOverlay.hidden = !ACTIVITY_GLYPHS[activity];
    if (normalized.caption) elements.caption.textContent = normalized.caption;
    if (normalized.thought && presence !== "home_sleeping") elements.thought.textContent = normalized.thought;
    if (activity || mood || prop) {
      const details = [normalized.activity, normalized.mood, normalized.prop].filter(Boolean).join(" · ");
      elements.companion.setAttribute("data-scene-label", details);
      const presenceLabel = elements.stage.getAttribute("aria-label") || "猫咪正在生活";
      elements.stage.setAttribute("aria-label", `${presenceLabel}；${details}`);
      elements.companion.setAttribute("aria-description", details);
    } else {
      elements.companion.removeAttribute("data-scene-label");
    }
  }

  function setIdentity(identity, accessories) {
    if (!identity) return;
    const identityKey = JSON.stringify([identity.coat, identity.eye_color, identity.marking, identity.signature, (accessories || []).map((item) => item.id)]);
    ["coat", "eye_color", "marking"].forEach((key) => {
      const value = identity[key];
      const datasetKey = key === "eye_color" ? "eye" : key;
      if (value) elements.companion.dataset[datasetKey] = value;
      else delete elements.companion.dataset[datasetKey];
    });
    const accent = (accessories || []).find((item) => item.id && item.id !== "sage_scarf");
    if (accent) elements.companion.dataset.accent = accent.id;
    else delete elements.companion.dataset.accent;
    const realisticSource = REALISTIC_CATS[identity.coat];
    if (identityKey === lastIdentityKey) return;
    lastIdentityKey = identityKey;
    if (realisticSource && elements.realisticImage) {
      elements.companion.classList.add("is-identity-pending");
      elements.realisticImage.onerror = clearRealisticArt;
      elements.realisticImage.onload = () => elements.companion.classList.remove("is-identity-pending");
      elements.realisticImage.src = realisticSource;
      elements.companion.classList.add("has-realistic-art");
    } else {
      elements.companion.classList.remove("has-realistic-art");
      elements.companion.classList.remove("is-identity-pending");
    }
    const coatNames = { orange_white: "橘白", cream_tabby: "奶油虎斑", silver_tabby: "银灰虎斑", black_white: "黑白" };
    const eyeNames = { amber: "琥珀色眼睛", green: "绿色眼睛", blue: "蓝色眼睛", hazel: "榛色眼睛" };
    const markingNames = { star: "星形额纹", stripe: "条纹额纹", cloud: "云朵额纹", heart: "心形额纹" };
    const identityLabel = [
      coatNames[identity.coat] || "独特毛色",
      eyeNames[identity.eye_color] || "独特眼睛",
      markingNames[identity.marking] || "特别印记",
    ].join("、");
    const accessoryLabel = (accessories || []).map((item) => item.name).filter(Boolean).join("、");
    if (elements.identityMeta) {
      elements.identityMeta.textContent = `身份已锁定 · ${identityLabel}${accessoryLabel ? ` · ${accessoryLabel}` : ""}`;
      elements.identityMeta.hidden = false;
    }
    if (identity.signature) elements.companion.dataset.identitySignature = identity.signature;
    elements.companion.setAttribute("data-identity-label", identityLabel);
    elements.companion.setAttribute("aria-description", identityLabel);
    elements.companion.setAttribute("aria-label", `一只${identityLabel}的旅行猫咪`);
  }

  const PHOTO_STORAGE_KEY = "cat:latestPhoto";

  function rememberPhoto(catId, url) {
    try {
      if (catId && url) window.localStorage.setItem(PHOTO_STORAGE_KEY, JSON.stringify({ cat: catId, url }));
      else window.localStorage.removeItem(PHOTO_STORAGE_KEY);
    } catch (_) {
      // Without storage the page still works; it only loses the first-paint hint.
    }
  }

  // Only reused for the cat the API has just confirmed, so switching accounts in one
  // browser never shows the previous account's cat.
  function rememberedPhotoFor(catId) {
    try {
      const stored = JSON.parse(window.localStorage.getItem(PHOTO_STORAGE_KEY) || "null");
      return stored && catId && stored.cat === catId && typeof stored.url === "string" ? stored.url : "";
    } catch (_) {
      return "";
    }
  }

  function settlePhotoPending() {
    elements.stage.classList.remove("is-photo-pending");
    delete document.documentElement.dataset.catPhotoPending;
  }

  function stopPhotoSwap() {
    window.clearTimeout(photoSwapTimer);
    photoSwapTimer = undefined;
    elements.stage.classList.remove("is-photo-transitioning");
  }

  function clearLatestPhoto(catId) {
    lastPhotoUrl = "";
    displayedPhotoUrl = "";
    photoLoadToken += 1;
    stopPhotoSwap();
    elements.stage.classList.remove("has-latest-photo");
    settlePhotoPending();
    rememberPhoto(catId, "");
  }

  // Shows one photo: the newest the cat has. `alternates` is only a failure chain —
  // an expired link falls through to the next candidate instead of leaving an empty stage.
  function setLatestPhoto(url, catId, alternates = []) {
    const nextUrl = typeof url === "string" ? url.trim() : "";
    if (!nextUrl || !elements.latestPhoto) {
      clearLatestPhoto(catId);
      return;
    }
    if (nextUrl === lastPhotoUrl) return;
    const previousDisplayedUrl = displayedPhotoUrl;
    lastPhotoUrl = nextUrl;
    const token = ++photoLoadToken;
    // While the cat is still hidden on first paint, keep it hidden until the photo
    // has loaded; a cat that is already visible is cross-faded once the photo arrives.
    const companionHidden = elements.companion.classList.contains("is-identity-pending")
      || Boolean(document.documentElement.dataset.catPhotoPending);
    if (companionHidden && !elements.stage.classList.contains("has-latest-photo")) {
      elements.stage.classList.add("is-photo-pending");
    }
    const preload = new Image();
    preload.onload = () => {
      if (token !== photoLoadToken) return;
      const applyPhoto = () => {
        if (token !== photoLoadToken) return;
        elements.latestPhoto.src = nextUrl;
        displayedPhotoUrl = nextUrl;
        elements.stage.classList.add("has-latest-photo");
        elements.stage.classList.remove("is-photo-transitioning");
        settlePhotoPending();
        rememberPhoto(catId, nextUrl);
      };
      if (previousDisplayedUrl && previousDisplayedUrl !== nextUrl && elements.stage.classList.contains("has-latest-photo")) {
        elements.stage.classList.add("is-photo-transitioning");
        window.clearTimeout(photoSwapTimer);
        photoSwapTimer = window.setTimeout(applyPhoto, 350);
      } else {
        applyPhoto();
      }
    };
    preload.onerror = () => {
      if (token !== photoLoadToken) return;
      stopPhotoSwap();
      if (alternates.length) {
        lastPhotoUrl = "";
        setLatestPhoto(alternates[0], catId, alternates.slice(1));
        return;
      }
      clearLatestPhoto(catId);
    };
    preload.src = nextUrl;
  }

  function setPresence(presence, trip) {
    const nextPresence = presence || "home_awake";
    const note = trip?.note?.zh || "";
    const changed = nextPresence !== lastPresence || note !== lastPresenceNote;
    elements.stage.dataset.presence = nextPresence;
    elements.companion.dataset.presence = nextPresence;
    if (changed) {
      elements.caption.textContent = PRESENCE_CAPTIONS[nextPresence] || PRESENCE_CAPTIONS.home_awake;
      elements.thought.textContent = PRESENCE_LABELS[nextPresence] || PRESENCE_LABELS.home_awake;
    }
    const presenceLabel = PRESENCE_LABELS[nextPresence] || PRESENCE_LABELS.home_awake;
    const identityLabel = elements.companion.dataset.identityLabel;
    elements.companion.setAttribute("aria-label", identityLabel ? `${presenceLabel}；${identityLabel}` : presenceLabel);
    elements.stage.setAttribute("aria-label", presenceLabel);
    if (note && (nextPresence === "away" || nextPresence === "returning_soon") && changed) elements.thought.textContent = note;
    if (changed) scheduleIdle();
    lastPresence = nextPresence;
    lastPresenceNote = note;
  }

  function revealPreview() {
    setLatestPhoto("");
    elements.companion.classList.remove("is-identity-pending");
  }

  scheduleIdle();
  return Object.freeze({ setIdentity, setLatestPhoto, rememberedPhotoFor, setPresence, setScene, react, showToast, revealPreview });
})();

const CatShell = (() => {
  const dialog = document.querySelector("#catDialog");
  const dialogEyebrow = document.querySelector("#dialogEyebrow");
  const dialogTitle = document.querySelector("#dialogTitle");
  const dialogBody = document.querySelector("#dialogBody");
  const dialogPrimary = document.querySelector("#dialogPrimary");
  const messagePreview = document.querySelector("#catMessagePreview");
  const presenceTitle = document.querySelector("#catPresenceTitle");
  const presenceMeta = document.querySelector("#catPresenceMeta");
  const todayPanel = document.querySelector("#catTodayPanel");
  const journalPanel = document.querySelector("#catJournalPanel");
  const atlasPanel = document.querySelector("#catAtlasPanel");
  const journalGrid = document.querySelector("#catJournalGrid");
  const atlasGrid = document.querySelector("#catAtlasGrid");
  let catState = null;
  let authState = "unknown";
  let dialogMode = "info";
  let adoptionPreview = null;
  let journalItems = [];

  async function api(path, options = {}) {
    const response = await fetch(path, {
      credentials: "same-origin",
      headers: { Accept: "application/json", ...(options.body ? { "Content-Type": "application/json" } : {}) },
      ...options,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(data.error?.message || `HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    return data;
  }

  function openDialog(title, body, eyebrow = "旅行猫咪", mode = "info") {
    dialogMode = mode;
    dialogEyebrow.textContent = eyebrow;
    dialogTitle.textContent = title;
    dialogBody.textContent = body;
    dialogPrimary.textContent = mode === "adopt" ? "确认领养" : "知道了";
    if (typeof dialog.showModal === "function" && !dialog.open) dialog.showModal();
    else CatScene.showToast(body);
  }

  function closeDialog() {
    if (dialog.open) dialog.close();
    dialogMode = "info";
  }

  function applyCatState(nextState) {
    catState = nextState;
    if (!nextState.cat) return;
    CatScene.setIdentity(nextState.cat.identity, nextState.cat.initial_accessories);
    const presence = Object.prototype.hasOwnProperty.call(nextState, "presence")
      ? nextState.presence
      : derivePresence(nextState.cat);
    const trip = nextState.cat.current_trip;
    CatScene.setPresence(presence, trip);
    const catId = nextState.cat.id;
    const scene = nextState.scene || nextState.cat.scene || trip?.scene;
    CatScene.setScene(scene, presence, trip);
    const messages = Array.isArray(nextState.messages) ? nextState.messages : null;
    // Messages arrive newest first, so the head of this list is the latest photo and the
    // rest only stand in when a link has expired.
    const photos = [...new Set([
      ...(messages || []).map((message) => message.payload?.postcard_url),
      scene?.photo_url,
      trip?.postcard_url,
      // /api/cat/state carries no messages; until /api/cat/home answers, fall back to
      // the photo this browser last showed for this same cat.
      messages ? "" : CatScene.rememberedPhotoFor(catId),
    ].filter((url) => typeof url === "string" && url.trim()))];
    CatScene.setLatestPhoto(photos[0] || "", catId, photos.slice(1));
    if (messages) renderPresence(messages[0], presence);
  }

  // The heading is the cat's own latest words, not a fixed label.
  function renderPresence(latest, presence) {
    const payload = latest?.payload || {};
    const text = (payload.text || payload.opening || payload.diary?.zh || "").trim();
    presenceTitle.textContent = text || PRESENCE_LABELS[presence] || PRESENCE_LABELS.home_awake;
    presenceMeta.textContent = latest
      ? `${relativeTime(latest.created_at)} · ${PRESENCE_SHORT[presence] || PRESENCE_SHORT.home_awake}`
      : PRESENCE_CAPTIONS[presence] || PRESENCE_CAPTIONS.home_awake;
  }

  function derivePresence(cat) {
    const status = cat?.current_trip?.status;
    if (status === "away" || status === "settling") return "away";
    const hour = new Date().getHours();
    return hour < 6 || hour >= 23 ? "home_sleeping" : "home_awake";
  }

  function identityText(preview) {
    const identity = preview.identity || {};
    const accessories = (preview.initial_accessories || []).map((item) => item.name).join("、");
    const coatNames = { orange_white: "橘白", cream_tabby: "奶油虎斑", silver_tabby: "银灰虎斑", black_white: "黑白" };
    const eyeNames = { amber: "琥珀色", green: "绿色", blue: "蓝色", hazel: "榛色" };
    return `固定身份：${coatNames[identity.coat] || "独特毛色"}，${eyeNames[identity.eye_color] || "独特眼睛"}，${identity.marking || "特别印记"}。初始配饰：${accessories || "专属配饰"}。确认后身份和初始配饰不会改变。`;
  }

  function escapeHtml(value) {
    return String(value ?? "")
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;")
      .replaceAll('"', "&quot;")
      .replaceAll("'", "&#39;");
  }

  function formatDate(value) {
    if (!value) return "刚刚";
    const date = new Date(value);
    return Number.isNaN(date.valueOf()) ? "刚刚" : date.toLocaleString([], { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }

  function relativeTime(value) {
    const date = new Date(value);
    if (Number.isNaN(date.valueOf())) return "刚刚";
    const minutes = Math.floor((Date.now() - date.getTime()) / 60_000);
    if (minutes < 1) return "刚刚";
    if (minutes < 60) return `${minutes} 分钟前`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours} 小时前`;
    const days = Math.floor(hours / 24);
    return days < 7 ? `${days} 天前` : formatDate(value);
  }

  // Scene messages carry a title that only repeats the kind label and the panel heading,
  // while a departure in the same "travel" stream carries a real one — so the title is
  // judged by what it says, not by the kind it belongs to.
  function messageView(message) {
    const payload = message.payload || {};
    const label = message.kind === "travel" ? "" : MESSAGE_LABELS[message.kind] || "猫咪记录";
    const title = (payload.title || "").trim();
    return {
      label,
      title: title && title !== label && !REDUNDANT_TITLES.has(title) ? title : "",
      text: payload.text || payload.opening || payload.diary?.zh || "猫咪把这段生活收进了日志。",
      photo: payload.postcard_url || "",
    };
  }

  function cardHtml(message, block, extraClass = "") {
    const view = messageView(message);
    const alt = view.title || view.text;
    return `<article class="${block}${extraClass}">
      ${view.photo ? `<div class="${block}__photo" data-photo-frame><img src="${escapeHtml(view.photo)}" alt="${escapeHtml(alt)}" loading="lazy"></div>` : ""}
      ${view.label ? `<div class="${block}__kind">${escapeHtml(view.label)}</div>` : ""}
      ${view.title ? `<h3>${escapeHtml(view.title)}</h3>` : ""}
      <p>${escapeHtml(view.text)}</p>
      <time datetime="${escapeHtml(message.created_at)}">${escapeHtml(relativeTime(message.created_at))}</time>
    </article>`;
  }

  function bindPhotoFallback(container) {
    container.querySelectorAll("[data-photo-frame] img").forEach((image) => {
      image.addEventListener("error", () => image.closest("[data-photo-frame]")?.remove(), { once: true });
    });
  }

  // The newest message is already the heading above the stage, so the list starts after it.
  function renderMessages(messages) {
    const rest = messages.slice(1);
    if (!rest.length) {
      messagePreview.innerHTML = `<div class="cat-empty">${messages.length ? "更早的旅迹还在路上。" : "猫咪正在慢慢生活，第一段旅迹还在路上。"}</div>`;
      return;
    }
    messagePreview.innerHTML = rest.map((message) => cardHtml(message, "cat-message-card")).join("");
    bindPhotoFallback(messagePreview);
  }

  function renderJournal(items, filter = "all") {
    const visible = filter === "all" ? items : items.filter((item) => {
      if (filter === "postcard") return item.kind === "postcard" || Boolean(item.payload?.postcard_url);
      return item.kind === filter;
    });
    if (!visible.length) {
      journalGrid.innerHTML = '<div class="cat-empty">这一类日志还没有记录。猫咪正在慢慢生活。</div>';
      return;
    }
    journalGrid.innerHTML = visible
      .map((message) => cardHtml(message, "cat-journal-card", message.kind === "postcard" ? " cat-journal-card--postcard" : ""))
      .join("");
    bindPhotoFallback(journalGrid);
  }

  function renderAtlas(data) {
    const accessories = data.accessories || [];
    const memories = data.memories || [];
    const memoryText = memories.slice(0, 12).map((memory) => `<span>${escapeHtml(memory.label || memory.memory_key)}</span>`).join("");
    const identity = catState?.cat?.identity || {};
    const identityNames = {
      coat: { orange_white: "橘白", cream_tabby: "奶油虎斑", silver_tabby: "银灰虎斑", black_white: "黑白" },
      eye_color: { amber: "琥珀色", green: "绿色", blue: "蓝色", hazel: "榛色" },
      marking: { star: "星形额纹", stripe: "条纹额纹", cloud: "云朵额纹", heart: "心形额纹" },
    };
    const identityText = [
      identityNames.coat[identity.coat] || "独特毛色",
      `${identityNames.eye_color[identity.eye_color] || "独特"}眼睛`,
      identityNames.marking[identity.marking] || "特别印记",
    ].join(" · ");
    const signature = identity.signature ? `<small>身份签名 · ${escapeHtml(identity.signature)}</small>` : "";
    atlasGrid.innerHTML = `<article class="cat-atlas-card cat-atlas-card--identity"><strong>固定身份</strong><p>${escapeHtml(identityText)}</p>${signature}</article>
      <article class="cat-atlas-card"><strong>成长配饰 · ${accessories.length}</strong><p>${accessories.length ? accessories.map((item) => escapeHtml(item.name || item.id)).join(" · ") : "猫咪还在积攒自己的旅途"}</p></article>
      <article class="cat-atlas-card"><strong>世界记忆 · ${memories.length}</strong><p class="cat-atlas-memory-list">${memoryText || "第一封信还在路上"}</p></article>`;
  }

  async function showAdoptionPreview(existingDraft = null) {
    if (authState !== "authenticated") {
      openDialog("登录后领养", "登录账户后，才能把这只猫咪的生活保存下来。", "旅行猫咪");
      return;
    }
    try {
      adoptionPreview = existingDraft || await api("/api/cat/adoption/preview", { method: "POST" });
      openDialog("确认领养你的猫咪", identityText(adoptionPreview), "第一次相遇", "adopt");
    } catch (error) {
      openDialog("暂时不能领养", error.message || "请稍后再试。", "旅行猫咪");
    }
  }

  async function confirmAdoption() {
    if (!adoptionPreview?.draft_id) return;
    dialogPrimary.disabled = true;
    try {
      const data = await api("/api/cat/adoption/confirm", {
        method: "POST",
        body: JSON.stringify({ draft_id: adoptionPreview.draft_id }),
      });
      applyCatState({ adopted: true, draft: null, cat: data });
      adoptionPreview = null;
      closeDialog();
      CatScene.react("猫咪来到你的猫窝了。", "我会记住我们的第一次相遇。 ");
    } catch (error) {
      if (error.status === 409) {
        adoptionPreview = null;
        await loadCatState();
        return;
      }
      openDialog("领养还没有完成", error.message || "请重新生成一份身份预览。", "旅行猫咪", "info");
    } finally {
      dialogPrimary.disabled = false;
    }
  }

  async function loadCatState() {
    try {
      const data = await api("/api/cat/state");
      authState = "authenticated";
      applyCatState(data);
      if (!data.adopted) {
        await showAdoptionPreview(data.draft);
        CatScene.revealPreview();
      }
      else await loadHome();
    } catch (error) {
      if (error.status === 401 || error.status === 403) {
        authState = "anonymous";
        catState = { adopted: false, cat: null, draft: null };
        CatScene.revealPreview();
      } else {
        CatScene.revealPreview();
        CatScene.showToast(error.message || "猫咪状态暂时无法读取");
      }
    }
  }

  async function loadHome() {
    if (authState !== "authenticated" || !catState?.adopted) return;
    try {
      // /api/cat/home already excludes retired guide messages, so nothing is filtered here.
      const home = await api("/api/cat/home");
      applyCatState({ ...home, adopted: true });
      renderMessages(home.messages || []);
    } catch (error) {
      CatScene.showToast(error.message || "猫窝状态暂时无法读取");
    }
  }

  function bind() {
    document.querySelectorAll("[data-journal-filter]").forEach((button) => {
      button.addEventListener("click", () => {
        document.querySelectorAll("[data-journal-filter]").forEach((item) => {
          const active = item === button;
          item.classList.toggle("is-active", active);
          item.setAttribute("aria-pressed", String(active));
        });
        renderJournal(journalItems, button.dataset.journalFilter || "all");
      });
    });
    document.querySelectorAll(".cat-nav-item").forEach((item) => {
      item.addEventListener("click", async (event) => {
        event.preventDefault();
        document.querySelectorAll(".cat-nav-item").forEach((nav) => {
          const active = nav === item;
          nav.classList.toggle("is-active", active);
          if (active) nav.setAttribute("aria-current", "page"); else nav.removeAttribute("aria-current");
        });
        if (authState !== "authenticated" || !catState?.adopted) {
          await showAdoptionPreview(catState?.draft);
          return;
        }
        todayPanel.hidden = item.hash !== "#home";
        journalPanel.hidden = item.hash !== "#journal";
        atlasPanel.hidden = item.hash !== "#atlas";
        if (item.hash === "#journal") {
          try {
            const data = await api("/api/cat/journal");
            journalItems = data.items || [];
            renderJournal(journalItems);
          } catch (error) { CatScene.showToast(error.message || "旅迹暂时无法读取"); }
        }
        if (item.hash === "#atlas") {
          try { renderAtlas(await api("/api/cat/atlas")); }
          catch (error) { CatScene.showToast(error.message || "图鉴暂时无法读取"); }
        }
        if (item.hash === "#home") window.scrollTo({ top: 0, behavior: "smooth" });
        else (item.hash === "#journal" ? journalPanel : atlasPanel)?.scrollIntoView({ behavior: "smooth", block: "start" });
      });
    });
    document.querySelector("#dialogClose")?.addEventListener("click", closeDialog);
    dialogPrimary?.addEventListener("click", () => {
      if (dialogMode === "adopt") confirmAdoption();
      else closeDialog();
    });
    dialog?.addEventListener("click", (event) => {
      if (event.target === dialog) closeDialog();
    });
  }

  async function init() {
    bind();
    renderMessages([]);
    await loadCatState();
    window.setInterval(() => {
      if (document.visibilityState === "visible" && authState === "authenticated" && catState?.adopted) loadHome();
    }, 60_000);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible" && authState === "authenticated" && catState?.adopted) loadHome();
    });
  }

  return Object.freeze({ init });
})();

document.addEventListener("DOMContentLoaded", () => CatShell.init());
