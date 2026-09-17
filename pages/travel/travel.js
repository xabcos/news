"use strict";

/*
 * Along's first page deliberately keeps three boundaries visible:
 *
 * - TravelScene owns only presentation and local interaction feedback.
 * - TravelShell wires the two together. Future account state, trip jobs and image
 *   generation can replace one boundary without changing the other two.
 */

// The stage and the "此刻" heading describe the same presence, so both read from here.
const PRESENCE_LABELS = Object.freeze({
  home_awake: "旅伴正在窗边休息",
  home_sleeping: "旅伴已经睡着了",
  away: "旅伴正在旅途中",
  returning_soon: "旅伴快要回来了",
  vigil: "旅伴在灯下安静地休息",
});
const PRESENCE_CAPTIONS = Object.freeze({
  home_awake: "在家 · 安静地生活",
  home_sleeping: "睡着了 · 房间守着温柔的夜",
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
const REDUNDANT_TITLES = new Set(["旅伴动态", "旅伴记录", "旅伴的一条记录"]);

const TravelScene = (() => {
  const elements = {
    stage: document.querySelector(".travel-stage"),
    companion: document.querySelector("#travelCompanion"),
    latestPhoto: document.querySelector("#travelLatestPhoto"),
    thought: document.querySelector("#travelThought"),
    caption: document.querySelector("#travelSceneCaption"),
    identityMeta: document.querySelector("#travelIdentityMeta"),
    toast: document.querySelector("#travelToast"),
    photoStatus: document.querySelector("#travelPhotoStatus"),
    photoStatusText: document.querySelector("#travelPhotoStatusText"),
    photoRetry: document.querySelector("#travelPhotoRetry"),
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
  const IDLE_ACTIONS = ["blink", "look", "stretch"];
  const ACTIVITY_GLYPHS = Object.freeze({
    observe: "⌁", packing: "⌂", reading: "✉", wandering: "↝", stretching: "⌒",
    sleeping: "☾", daydreaming: "·",
  });
  const PROP_GLYPHS = Object.freeze({ scarf: "▰", satchel: "▱", suitcase: "▣", letter: "✉", telescope: "◌" });

  const sceneOverlay = document.createElement("span");
  sceneOverlay.className = "travel-scene-overlay";
  sceneOverlay.setAttribute("aria-hidden", "true");
  const motionOverlay = document.createElement("span");
  motionOverlay.className = "travel-scene-motion";
  motionOverlay.setAttribute("aria-hidden", "true");
  elements.companion?.append(sceneOverlay, motionOverlay);

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
      const presenceLabel = elements.stage.getAttribute("aria-label") || "旅伴正在生活";
      elements.stage.setAttribute("aria-label", `${presenceLabel}；${details}`);
      elements.companion.setAttribute("aria-description", details);
    } else {
      elements.companion.removeAttribute("data-scene-label");
    }
  }

  function setIdentity(identity, accessories) {
    if (!identity) return;
    elements.companion.classList.remove("is-identity-pending");
    const identityKey = JSON.stringify([identity.name_seed, identity.hair, identity.skin_tone, identity.silhouette, identity.style, identity.signature, (accessories || []).map((item) => item.id)]);
    ["hair", "skin_tone", "silhouette", "style"].forEach((key) => {
      const value = identity[key];
      if (value) elements.companion.dataset[key] = value;
      else delete elements.companion.dataset[key];
    });
    const accent = (accessories || []).find((item) => item.id);
    if (accent) elements.companion.dataset.accent = accent.id;
    else delete elements.companion.dataset.accent;
    if (identityKey === lastIdentityKey) return;
    lastIdentityKey = identityKey;
    const identityLabel = [
      identity.name_seed || "旅伴",
      identity.appearance_summary || "属于这个人的面容与气质",
    ].join(" · ");
    const accessoryLabel = (accessories || []).map((item) => item.name).filter(Boolean).join("、");
    if (elements.identityMeta) {
      elements.identityMeta.textContent = `身份已锁定 · ${identityLabel}${accessoryLabel ? ` · ${accessoryLabel}` : ""}`;
      elements.identityMeta.hidden = false;
    }
    if (identity.signature) elements.companion.dataset.identitySignature = identity.signature;
    elements.companion.setAttribute("data-identity-label", identityLabel);
    elements.companion.setAttribute("aria-description", identityLabel);
    elements.companion.setAttribute("aria-label", `${identityLabel}的旅伴`);
  }

  const PHOTO_STORAGE_KEY = "travel:latestPhoto";

  function rememberPhoto(travelId, url) {
    try {
      if (travelId && url) window.localStorage.setItem(PHOTO_STORAGE_KEY, JSON.stringify({ travel: travelId, url }));
      else window.localStorage.removeItem(PHOTO_STORAGE_KEY);
    } catch (_) {
      // Without storage the page still works; it only loses the first-paint hint.
    }
  }

  // Only reused for the travel the API has just confirmed, so switching accounts in one
  // browser never shows the previous account's travel.
  function rememberedPhotoFor(travelId) {
    try {
      const stored = JSON.parse(window.localStorage.getItem(PHOTO_STORAGE_KEY) || "null");
      return stored && travelId && stored.travel === travelId && typeof stored.url === "string" ? stored.url : "";
    } catch (_) {
      return "";
    }
  }

  function settlePhotoPending() {
    elements.stage.classList.remove("is-photo-pending");
    delete document.documentElement.dataset.travelPhotoPending;
  }

  function photoStatus(message, retry = false) {
    elements.photoStatus.hidden = !message;
    elements.photoStatusText.textContent = message;
    elements.photoRetry.hidden = !retry;
  }

  function stopPhotoSwap() {
    window.clearTimeout(photoSwapTimer);
    photoSwapTimer = undefined;
    elements.stage.classList.remove("is-photo-transitioning");
  }

  function clearLatestPhoto(travelId) {
    lastPhotoUrl = "";
    displayedPhotoUrl = "";
    photoLoadToken += 1;
    stopPhotoSwap();
    elements.stage.classList.remove("has-latest-photo");
    elements.stage.style.removeProperty("--travel-photo-ratio");
    elements.latestPhoto.removeAttribute("src");
    elements.companion.classList.remove("is-identity-pending");
    settlePhotoPending();
    rememberPhoto(travelId, "");
    photoStatus(travelId ? "正在准备第一张照片，完成后会自动显示。" : "登录后，开启你们的旅途。");
  }

  // Shows one photo: the newest the travel has. `alternates` is only a failure chain —
  // an expired link falls through to the next candidate instead of leaving an empty stage.
  function setLatestPhoto(url, travelId, alternates = []) {
    const nextUrl = typeof url === "string" ? url.trim() : "";
    if (!nextUrl || !elements.latestPhoto) {
      clearLatestPhoto(travelId);
      return;
    }
    if (nextUrl === lastPhotoUrl) return;
    const previousDisplayedUrl = displayedPhotoUrl;
    lastPhotoUrl = nextUrl;
    photoStatus("正在加载旅伴的照片…");
    const token = ++photoLoadToken;
    // While the travel is still hidden on first paint, keep it hidden until the photo
    // has loaded; a travel that is already visible is cross-faded once the photo arrives.
    const companionHidden = elements.companion.classList.contains("is-identity-pending")
      || Boolean(document.documentElement.dataset.travelPhotoPending);
    if (companionHidden && !elements.stage.classList.contains("has-latest-photo")) {
      elements.stage.classList.add("is-photo-pending");
    }
    const preload = new Image();
    preload.onload = () => {
      if (token !== photoLoadToken) return;
      const applyPhoto = () => {
        if (token !== photoLoadToken) return;
        elements.stage.style.setProperty("--travel-photo-ratio", String(preload.naturalWidth / preload.naturalHeight));
        elements.latestPhoto.src = nextUrl;
        displayedPhotoUrl = nextUrl;
        elements.stage.classList.add("has-latest-photo");
        elements.stage.classList.remove("is-photo-transitioning");
        settlePhotoPending();
        rememberPhoto(travelId, nextUrl);
        photoStatus("");
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
        setLatestPhoto(alternates[0], travelId, alternates.slice(1));
        return;
      }
      clearLatestPhoto(travelId);
      photoStatus("照片暂时加载失败，可以重新加载。", true);
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

const TravelShell = (() => {
  const messagePreview = document.querySelector("#travelMessagePreview");
  const presenceTitle = document.querySelector("#travelPresenceTitle");
  const presenceMeta = document.querySelector("#travelPresenceMeta");
  const todayPanel = document.querySelector("#travelTodayPanel");
  const journalPanel = document.querySelector("#travelJournalPanel");
  const atlasPanel = document.querySelector("#travelAtlasPanel");
  const worldPanel = document.querySelector("#travelWorldPanel");
  const journalGrid = document.querySelector("#travelJournalGrid");
  const atlasGrid = document.querySelector("#travelAtlasGrid");
  let travelState = null;
  let authState = "unknown";
  let adoptionPreview = null;
  let journalItems = [];
  const onboarding = document.querySelector("#alongOnboarding");
  const createForm = document.querySelector("#alongCreateForm");
  const waiting = document.querySelector("#alongWaiting");
  const firstMeeting = document.querySelector("#alongFirstMeeting");
  const createButton = document.querySelector("#alongCreate");
  const confirmButton = document.querySelector("#alongConfirm");
  const portrait = document.querySelector("#alongPortrait");
  const errorBox = document.querySelector("#alongError");
  const uploadInput = document.querySelector("#alongPhotos");
  const photoList = document.querySelector("#alongPhotoList");
  const sourcePhotos = document.querySelector("#alongUploadSection");
  let selectedPhotos = [];
  let draftEpoch = 0;
  let draftTimer;
  let activeDraftId = null;
  let submitting = false;
  let confirming = false;
  let portraitLoaded = false;
  let homeLoading = false;
  let worldLoading = false;
  let socialSaving = false;
  let worldEpoch = 0;

  async function api(path, options = {}) {
    const response = await fetch(path, {
      credentials: "same-origin",
      headers: { Accept: "application/json", ...(options.body && !(options.body instanceof FormData) ? { "Content-Type": "application/json" } : {}) },
      ...options,
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(data.error?.message || (typeof data.error === "string" ? data.error : "") || `HTTP ${response.status}`);
      error.status = response.status;
      throw error;
    }
    return data;
  }

  function applyTravelState(nextState) {
    travelState = nextState;
    if (!nextState.companion) return;
    showLivingSpace();
    TravelScene.setIdentity(nextState.companion.identity, nextState.companion.initial_accessories);
    const presence = Object.prototype.hasOwnProperty.call(nextState, "presence")
      ? nextState.presence
      : derivePresence(nextState.companion);
    const trip = nextState.companion.current_trip;
    TravelScene.setPresence(presence, trip);
    const travelId = nextState.companion.id;
    const scene = nextState.scene || nextState.companion.scene || trip?.scene;
    TravelScene.setScene(scene, presence, trip);
    const messages = Array.isArray(nextState.messages) ? nextState.messages : null;
    // Messages arrive newest first, so the head of this list is the latest photo and the
    // rest only stand in when a link has expired.
    const photos = [...new Set([
      ...(messages || []).map((message) => message.payload?.postcard_url),
      scene?.photo_url,
      nextState.companion?.portrait_url,
      // /api/travel/state carries no messages; until /api/travel/home answers, fall back to
      // the photo this browser last showed for this same travel.
      messages ? "" : TravelScene.rememberedPhotoFor(travelId),
    ].filter((url) => typeof url === "string" && url.trim()))];
    TravelScene.setLatestPhoto(photos[0] || "", travelId, photos.slice(1));
    if (messages) renderPresence(messages[0], presence);
  }

  // The heading is the travel's own latest words, not a fixed label.
  function renderPresence(latest, presence) {
    const payload = latest?.payload || {};
    const text = (payload.text || payload.opening || payload.diary?.zh || "").trim();
    presenceTitle.textContent = text || PRESENCE_LABELS[presence] || PRESENCE_LABELS.home_awake;
    presenceMeta.textContent = latest
      ? `${relativeTime(latest.created_at)} · ${PRESENCE_SHORT[presence] || PRESENCE_SHORT.home_awake}`
      : PRESENCE_CAPTIONS[presence] || PRESENCE_CAPTIONS.home_awake;
  }

  function derivePresence(travel) {
    const status = travel?.current_trip?.status;
    if (status === "away" || status === "settling") return "away";
    const hour = new Date().getHours();
    return hour < 6 || hour >= 23 ? "home_sleeping" : "home_awake";
  }

  function identityText(preview) {
    const identity = preview.identity || {};
    return identity.appearance_summary || identity.personality_summary || "有自己的模样、气质与审美，也将拥有与你共同的回忆。";
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
    const label = message.kind === "travel" ? "" : MESSAGE_LABELS[message.kind] || "旅伴记录";
    const title = (payload.title || "").trim();
    return {
      label,
      title: title && title !== label && !REDUNDANT_TITLES.has(title) ? title : "",
      text: payload.text || payload.opening || payload.diary?.zh || "旅伴把这段生活收进了日志。",
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
      messagePreview.innerHTML = `<div class="travel-empty">${messages.length ? "更早的旅迹还在路上。" : "旅伴正在慢慢生活，第一段旅迹还在路上。"}</div>`;
      return;
    }
    messagePreview.innerHTML = rest.map((message) => cardHtml(message, "travel-message-card")).join("");
    bindPhotoFallback(messagePreview);
  }

  function renderJournal(items, filter = "all") {
    const visible = filter === "all" ? items : items.filter((item) => {
      if (filter === "postcard") return item.kind === "postcard" || Boolean(item.payload?.postcard_url);
      return item.kind === filter;
    });
    if (!visible.length) {
      journalGrid.innerHTML = '<div class="travel-empty">这一类日志还没有记录。旅伴正在慢慢生活。</div>';
      return;
    }
    journalGrid.innerHTML = visible
      .map((message) => cardHtml(message, "travel-journal-card", message.kind === "postcard" ? " travel-journal-card--postcard" : ""))
      .join("");
    bindPhotoFallback(journalGrid);
  }

  function renderAtlas(data) {
    const accessories = data.accessories || [];
    const memories = data.memories || [];
    const memoryText = memories.slice(0, 12).map((memory) => `<span>${escapeHtml(memory.label || memory.memory_key)}</span>`).join("");
    const identity = travelState?.companion?.identity || {};
    const identityText = [identity.name_seed, identity.appearance_summary].filter(Boolean).join(" · ") || "旅伴的固定形象";
    const signature = identity.signature ? `<small>身份签名 · ${escapeHtml(identity.signature)}</small>` : "";
    atlasGrid.innerHTML = `<article class="travel-atlas-card travel-atlas-card--identity"><strong>固定身份</strong><p>${escapeHtml(identityText)}</p>${signature}</article>
      <article class="travel-atlas-card"><strong>成长配饰 · ${accessories.length}</strong><p>${accessories.length ? accessories.map((item) => escapeHtml(item.name || item.id)).join(" · ") : "旅伴还在积攒自己的旅途"}</p></article>
      <article class="travel-atlas-card"><strong>世界记忆 · ${memories.length}</strong><p class="travel-atlas-memory-list">${memoryText || "第一封信还在路上"}</p></article>`;
  }

  function setOnboardingError(message = "") {
    errorBox.textContent = message;
    errorBox.hidden = !message;
  }

  function stopDraftPolling() {
    window.clearTimeout(draftTimer);
    draftTimer = undefined;
    draftEpoch += 1;
  }

  function showLivingSpace() {
    onboarding.hidden = true;
    document.body.classList.remove("is-meeting");
    document.querySelector("#travelLivingSpace").hidden = false;
    document.querySelector(".travel-bottom-nav").hidden = false;
    if (![journalPanel, atlasPanel, worldPanel].some((panel) => !panel.hidden)) todayPanel.hidden = false;
  }

  function showOnboarding() {
    onboarding.hidden = false;
    document.body.classList.add("is-meeting");
    document.querySelector("#travelLivingSpace").hidden = true;
    document.querySelector(".travel-bottom-nav").hidden = true;
    [todayPanel, journalPanel, atlasPanel, worldPanel].forEach((panel) => { panel.hidden = true; });
    const anonymous = authState !== "authenticated";
    document.querySelector("#alongAuth").hidden = !anonymous;
    createForm.hidden = anonymous;
    waiting.hidden = true;
    firstMeeting.hidden = true;
  }

  function editMeeting() {
    if (submitting || confirming) return;
    stopDraftPolling();
    adoptionPreview = null;
    activeDraftId = null;
    portraitLoaded = false;
    portrait.removeAttribute("src");
    showOnboarding();
    setOnboardingError();
    updateCreateState();
    createForm.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function sourceValue() { return createForm.elements.source.value; }

  function updateCreateState() {
    const needsPhotos = sourceValue() === "photos";
    sourcePhotos.hidden = !needsPhotos;
    createButton.disabled = submitting || !createForm.elements.gender.value
      || (needsPhotos && (!selectedPhotos.length || !document.querySelector("#alongConsent").checked));
    document.querySelector("#alongPhotoCount").textContent = `${selectedPhotos.length} / 7 张`;
    uploadInput.disabled = selectedPhotos.length >= 7 || submitting;
    document.querySelector("#alongUploadLabel").hidden = selectedPhotos.length >= 7;
  }

  function renderSelectedPhotos() {
    photoList.innerHTML = selectedPhotos.map((item, index) => `<article class="along-photo-tile">
      <img src="${escapeHtml(item.url)}" alt="${index === 0 ? "主形象" : `补充照片 ${index + 1}`}：${escapeHtml(item.file.name)}">
      <span class="along-photo-badge">${index === 0 ? "主形象" : `补充 ${index}`}</span>
      <button class="along-photo-remove" type="button" data-remove-photo="${index}" aria-label="移除照片 ${index + 1}">×</button>
      ${index ? `<button class="along-photo-promote" type="button" data-promote-photo="${index}">设为主图</button>` : ""}
    </article>`).join("");
    updateCreateState();
  }

  function choosePhotos(files) {
    const candidates = Array.from(files || []);
    uploadInput.value = "";
    if (selectedPhotos.length + candidates.length > 7) {
      setOnboardingError("最多保留 7 张形象照片，请减少本次选择的数量。");
      return;
    }
    const supported = new Set(["image/jpeg", "image/png", "image/webp"]);
    if (candidates.some((file) => !supported.has(file.type))) {
      setOnboardingError("请选择 JPEG、PNG 或 WebP 格式的照片。");
      return;
    }
    if (candidates.some((file) => file.size > 10 * 1024 * 1024 || file.size === 0)) {
      setOnboardingError("每张照片需大于 0 字节且不超过 10 MB。");
      return;
    }
    const total = [...selectedPhotos.map((item) => item.file), ...candidates].reduce((sum, file) => sum + file.size, 0);
    if (total > 70 * 1024 * 1024) {
      setOnboardingError("照片总大小不能超过 70 MB。");
      return;
    }
    setOnboardingError();
    selectedPhotos.push(...candidates.map((file) => ({ file, url: URL.createObjectURL(file) })));
    renderSelectedPhotos();
  }

  function showWaiting(message) {
    createForm.hidden = true;
    firstMeeting.hidden = true;
    waiting.hidden = false;
    document.querySelector("#alongWaitingText").textContent = message || "正在构思独特的人物，并准备第一张照片。通常需要几分钟。";
  }

  function applyDraft(draft, epoch) {
    if (epoch !== draftEpoch || travelState?.adopted) return;
    if (!draft || draft.status === "empty") {
      editMeeting();
      return;
    }
    if (activeDraftId && draft.draft_id && activeDraftId !== draft.draft_id) {
      stopDraftPolling();
      showWaiting("这次初遇已在其他页面更新。返回调整后，可以重新选择想遇见的人。");
      return;
    }
    activeDraftId = draft.draft_id || activeDraftId;
    if (draft.status === "failed") {
      stopDraftPolling();
      showWaiting("这次初遇没有顺利完成。你的旅伴尚未确认，可以返回调整后重新创造。");
      const message = typeof draft.error === "string" ? draft.error : draft.error?.message;
      setOnboardingError(message || "形象暂未生成，请稍后再试。");
      return;
    }
    if (draft.status === "ready" && draft.identity && draft.portrait_url) {
      window.clearTimeout(draftTimer);
      adoptionPreview = draft;
      createForm.hidden = true;
      waiting.hidden = true;
      firstMeeting.hidden = false;
      portraitLoaded = false;
      confirmButton.disabled = true;
      confirmButton.textContent = "照片加载中…";
      document.querySelector("#alongMeetName").textContent = draft.identity.name_seed || draft.identity.name || "你好，旅伴";
      document.querySelector("#alongMeetDescription").textContent = identityText(draft);
      document.querySelector("#alongPortraitRetry").hidden = true;
      portrait.alt = `${draft.identity.name_seed || draft.identity.name || "旅伴"}的初见形象`;
      portrait.onload = () => {
        if (epoch !== draftEpoch || adoptionPreview?.draft_id !== draft.draft_id) return;
        portraitLoaded = true;
        confirmButton.disabled = false;
        confirmButton.textContent = "开始同行";
        document.querySelector("#alongPortraitRetry").hidden = true;
        setOnboardingError();
      };
      portrait.onerror = () => {
        if (epoch !== draftEpoch) return;
        portraitLoaded = false;
        confirmButton.disabled = true;
        confirmButton.textContent = "先看看初见照片";
        document.querySelector("#alongPortraitRetry").hidden = false;
        setOnboardingError("初见照片暂时未加载成功，请重新加载后再开始同行。");
      };
      portrait.src = draft.portrait_url;
      return;
    }
    showWaiting(draft.stage === "portrait"
      ? "人物设定已完成，正在创作并核对初见照片。高质量形象可能需要十几分钟，无需重复提交。"
      : "正在理解你的期待、构思人物的模样与性格。完成后会继续准备初见照片。");
    scheduleDraftPoll(epoch);
  }

  function scheduleDraftPoll(epoch, delay = 4000) {
    window.clearTimeout(draftTimer);
    draftTimer = window.setTimeout(async () => {
      if (epoch !== draftEpoch || travelState?.adopted) return;
      if (document.visibilityState !== "visible") { scheduleDraftPoll(epoch, 8000); return; }
      try {
        const draft = await api("/api/travel/adoption/preview");
        if (epoch !== draftEpoch) return;
        setOnboardingError();
        applyDraft(draft, epoch);
      } catch (error) {
        if (epoch !== draftEpoch) return;
        if (error.status === 409) {
          stopDraftPolling();
          await loadTravelState();
          return;
        }
        if (error.status === 401 || error.status === 403) {
          stopDraftPolling();
          authState = "anonymous";
          showOnboarding();
          return;
        }
        setOnboardingError("暂时没有收到更新，正在重新连接。不会重复创造人物。");
        scheduleDraftPoll(epoch, 8000);
      }
    }, delay);
  }

  async function createMeeting(event) {
    event.preventDefault();
    if (submitting || travelState?.adopted || authState !== "authenticated") return;
    if (!createForm.elements.gender.value) { setOnboardingError("先选择想遇见的他或她。"); return; }
    if (sourceValue() === "photos" && (!selectedPhotos.length || !document.querySelector("#alongConsent").checked)) {
      setOnboardingError("请选择 1–7 张照片，并确认你有权使用这些照片。");
      return;
    }
    stopDraftPolling();
    const epoch = draftEpoch;
    activeDraftId = null;
    adoptionPreview = null;
    setOnboardingError();
    submitting = true;
    updateCreateState();
    document.querySelector("#alongBackToEdit").disabled = true;
    const body = new FormData();
    body.set("source", sourceValue());
    body.set("gender", createForm.elements.gender.value);
    body.set("name", document.querySelector("#alongName").value.trim());
    body.set("description", document.querySelector("#alongDescription").value.trim());
    if (sourceValue() === "photos") {
      body.set("consent", "true");
      selectedPhotos.forEach((item) => body.append("photos", item.file, item.file.name));
    }
    showWaiting(sourceValue() === "photos" ? "正在保存形象照片，准备你们的第一次相遇…" : "正在准备这次相遇…");
    try {
      const draft = await api("/api/travel/adoption/preview", { method: "POST", body });
      if (epoch === draftEpoch) applyDraft(draft, epoch);
    } catch (error) {
      if (epoch !== draftEpoch) return;
      if (error.status === 409) {
        await loadTravelState();
      } else if (!error.status || error.status >= 500) {
        // An interrupted POST may already have started a job. Read before offering another creation.
        showWaiting("正在确认生成进度。无需再次点击创造相遇。");
        scheduleDraftPoll(epoch, 1000);
      } else {
        createForm.hidden = false;
        waiting.hidden = true;
        setOnboardingError(error.message || "暂时无法创造这次相遇，请稍后再试。");
      }
    } finally {
      submitting = false;
      document.querySelector("#alongBackToEdit").disabled = false;
      updateCreateState();
    }
  }

  async function confirmAdoption() {
    if (confirming || !portraitLoaded || !adoptionPreview?.draft_id) return;
    confirming = true;
    confirmButton.disabled = true;
    confirmButton.textContent = "正在记住这次初遇…";
    document.querySelector("#alongEditPreview").disabled = true;
    try {
      const data = await api("/api/travel/adoption/confirm", {
        method: "POST", body: JSON.stringify({ draft_id: adoptionPreview.draft_id }),
      });
      stopDraftPolling();
      applyTravelState({ adopted: true, draft: null, companion: data });
      adoptionPreview = null;
      selectedPhotos.forEach((item) => URL.revokeObjectURL(item.url));
      selectedPhotos = [];
      TravelScene.react("从今天起，一起看世界。", "我会记住我们的第一次相遇。");
      await loadHome();
    } catch (error) {
      if (error.status === 409) { await loadTravelState(); return; }
      setOnboardingError(error.message || "还没有完成相识，请再试一次。");
    } finally {
      confirming = false;
      confirmButton.disabled = !portraitLoaded;
      confirmButton.textContent = "开始同行";
      document.querySelector("#alongEditPreview").disabled = false;
    }
  }

  async function loadTravelState() {
    try {
      const data = await api("/api/travel/state");
      authState = "authenticated";
      applyTravelState(data);
      if (!data.adopted) {
        TravelScene.revealPreview();
        showOnboarding();
        stopDraftPolling();
        activeDraftId = null;
        if (data.draft && data.draft.status !== "empty") applyDraft(data.draft, draftEpoch);
        updateCreateState();
      } else {
        stopDraftPolling();
        await loadHome();
      }
    } catch (error) {
      TravelScene.revealPreview();
      if (error.status === 401 || error.status === 403) {
        authState = "anonymous";
        travelState = { adopted: false, companion: null, draft: null };
        showOnboarding();
      } else {
        TravelScene.showToast(error.message || "旅伴状态暂时无法读取");
        document.querySelector("#travelPhotoStatusText").textContent = "暂时无法读取近况，请重新加载。";
        document.querySelector("#travelPhotoRetry").hidden = false;
      }
    }
  }

  async function loadHome() {
    if (homeLoading || authState !== "authenticated" || !travelState?.adopted) return;
    homeLoading = true;
    try {
      // /api/travel/home already excludes retired guide messages, so nothing is filtered here.
      const home = await api("/api/travel/home");
      applyTravelState({ ...home, adopted: true });
      renderMessages(home.messages || []);
    } catch (error) {
      TravelScene.showToast(error.message || "旅居空间状态暂时无法读取");
    } finally { homeLoading = false; }
  }

  function renderWorld(data) {
    const enabled = data.settings?.social_mode === "open";
    const toggle = document.querySelector("#alongSocialEnabled");
    if (!socialSaving) { toggle.checked = enabled; toggle.disabled = false; }
    const relationships = data.relationships || [];
    document.querySelector("#alongFriends").innerHTML = relationships.length
      ? `<h3>熟悉的朋友</h3><div class="along-friend-list">${relationships.map((friend) => `<article class="along-friend"><strong>${escapeHtml(friend.name || "一位朋友")}</strong><small>相遇 ${escapeHtml(friend.meetings || 1)} 次</small><p>${escapeHtml(friend.memory || "共同的回忆，还在慢慢积攒。")}</p></article>`).join("")}</div>` : "";
    const events = data.shared_events || [];
    document.querySelector("#alongWorldStatus").textContent = enabled ? "世界已向新朋友敞开；相遇也有自己的时机。" : "当前只属于你们。开启自主结伴，遇见更多同行的人。";
    document.querySelector("#alongSharedEvents").innerHTML = events.length ? events.map((event) => {
      const names = (event.participants || []).map((person) => person.name).filter(Boolean).join(" · ");
      const photo = event.photo?.url;
      const pending = event.status === "photo_failed" ? "共同经历已珍藏，合影暂未完成。" : "共同经历已珍藏，合影正在准备。";
      return `<article class="travel-journal-card along-shared-card">${photo ? `<div class="travel-journal-card__photo" data-photo-frame><img src="${escapeHtml(photo)}" alt="${escapeHtml(names)}的共同旅行合影" loading="lazy"></div>` : `<div class="along-photo-pending">${pending}</div>`}<div class="travel-journal-card__kind">${escapeHtml(names)}</div><h3>${escapeHtml(event.story?.title || "共同的旅途")}</h3><p>${escapeHtml(event.story?.text_zh || event.story?.activity || "这一刻，成为了彼此的共同回忆。")}</p><time datetime="${escapeHtml(event.happened_at)}">${escapeHtml([event.location, relativeTime(event.happened_at)].filter(Boolean).join(" · "))}</time></article>`;
    }).join("") : '<div class="travel-empty">下一次相遇，或许就在下一段旅途。</div>';
    bindPhotoFallback(document.querySelector("#alongSharedEvents"));
  }

  async function loadWorld() {
    if (worldLoading || socialSaving || !travelState?.adopted) return;
    const epoch = worldEpoch;
    worldLoading = true;
    try {
      const data = await api("/api/travel/world");
      if (epoch === worldEpoch && !socialSaving) renderWorld(data);
    } catch (error) {
      if (epoch === worldEpoch) document.querySelector("#alongWorldStatus").textContent = error.message || "共同世界暂时无法读取。";
    } finally {
      worldLoading = false;
      if (epoch !== worldEpoch && !socialSaving) void loadWorld();
    }
  }

  function bindMeeting() {
    createForm.addEventListener("submit", createMeeting);
    createForm.addEventListener("change", updateCreateState);
    uploadInput.addEventListener("change", () => choosePhotos(uploadInput.files));
    photoList.addEventListener("click", (event) => {
      if (submitting) return;
      const remove = event.target.closest("[data-remove-photo]");
      const promote = event.target.closest("[data-promote-photo]");
      if (remove) {
        const removed = selectedPhotos.splice(Number(remove.dataset.removePhoto), 1)[0];
        if (removed) URL.revokeObjectURL(removed.url);
        renderSelectedPhotos();
      } else if (promote) {
        const chosen = selectedPhotos.splice(Number(promote.dataset.promotePhoto), 1)[0];
        if (chosen) selectedPhotos.unshift(chosen);
        renderSelectedPhotos();
      }
    });
    document.querySelector("#alongBackToEdit").addEventListener("click", editMeeting);
    document.querySelector("#alongEditPreview").addEventListener("click", editMeeting);
    document.querySelector("#alongConfirm").addEventListener("click", confirmAdoption);
    document.querySelector("#alongPortraitRetry").addEventListener("click", () => {
      if (!adoptionPreview?.portrait_url) return;
      portrait.removeAttribute("src");
      portrait.src = adoptionPreview.portrait_url;
    });
    document.querySelector("#alongSocialEnabled").addEventListener("change", async (event) => {
      if (socialSaving) return;
      const toggle = event.target;
      const desired = toggle.checked;
      socialSaving = true;
      worldEpoch += 1;
      toggle.disabled = true;
      try {
        const result = await api("/api/travel/world/settings", {
          method: "POST", body: JSON.stringify({ social_mode: desired ? "open" : "private" }),
        });
        toggle.checked = result.settings?.social_mode === "open";
        TravelScene.showToast(toggle.checked ? "世界已向新朋友敞开。" : "已暂停新的自主结伴。共同回忆仍会保留。");
      } catch (error) {
        toggle.checked = !desired;
        TravelScene.showToast(error.message || "暂时无法保存，请稍后再试。");
      } finally {
        socialSaving = false;
        toggle.disabled = false;
        await loadWorld();
      }
    });
    updateCreateState();
  }

  function bind() {
    document.querySelector("#travelPhotoRetry")?.addEventListener("click", () => travelState?.adopted ? loadHome() : loadTravelState());
    bindMeeting();
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
    document.querySelectorAll(".travel-nav-item").forEach((item) => {
      item.addEventListener("click", async (event) => {
        event.preventDefault();
        document.querySelectorAll(".travel-nav-item").forEach((nav) => {
          const active = nav === item;
          nav.classList.toggle("is-active", active);
          if (active) nav.setAttribute("aria-current", "page"); else nav.removeAttribute("aria-current");
        });
        if (authState !== "authenticated" || !travelState?.adopted) {
          showOnboarding();
          return;
        }
        todayPanel.hidden = item.hash !== "#home";
        journalPanel.hidden = item.hash !== "#journal";
        atlasPanel.hidden = item.hash !== "#atlas";
        worldPanel.hidden = item.hash !== "#world";
        if (item.hash === "#journal") {
          try {
            const data = await api("/api/travel/journal");
            journalItems = data.items || [];
            renderJournal(journalItems);
          } catch (error) { TravelScene.showToast(error.message || "旅迹暂时无法读取"); }
        }
        if (item.hash === "#atlas") {
          try { renderAtlas(await api("/api/travel/atlas")); }
          catch (error) { TravelScene.showToast(error.message || "图鉴暂时无法读取"); }
        }
        if (item.hash === "#world") await loadWorld();
        if (item.hash === "#home") window.scrollTo({ top: 0, behavior: "smooth" });
        else (item.hash === "#journal" ? journalPanel : item.hash === "#world" ? worldPanel : atlasPanel)?.scrollIntoView({ behavior: "smooth", block: "start" });
      });
    });

  }

  async function init() {
    bind();
    renderMessages([]);
    await loadTravelState();
    window.setInterval(() => {
      if (document.visibilityState === "visible" && authState === "authenticated" && travelState?.adopted) loadHome();
      if (document.visibilityState === "visible" && !worldPanel.hidden) loadWorld();
    }, 15_000);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible" && authState === "authenticated" && travelState?.adopted) loadHome();
    });
  }

  return Object.freeze({ init });
})();

document.addEventListener("DOMContentLoaded", () => TravelShell.init());
