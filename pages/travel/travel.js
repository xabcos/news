"use strict";

/*
 * Travel Girl's first page deliberately keeps three boundaries visible:
 *
 * - TravelScene owns only presentation and local interaction feedback.
 * - TravelShell wires the two together. Future account state, trip jobs and image
 *   generation can replace one boundary without changing the other two.
 */

// The stage and the "此刻" heading describe the same presence, so both read from here.
const PRESENCE_LABELS = Object.freeze({
  home_awake: "旅行女孩正在窗边休息",
  home_sleeping: "旅行女孩已经睡着了",
  away: "旅行女孩正在旅途中",
  returning_soon: "旅行女孩快要回来了",
  vigil: "旅行女孩在灯下安静地休息",
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
const REDUNDANT_TITLES = new Set(["旅行女孩动态", "旅行女孩记录", "旅行女孩的一条记录"]);
const VISUAL_VERSION = "flare-youth-v1";

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
      const presenceLabel = elements.stage.getAttribute("aria-label") || "旅行女孩正在生活";
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
      identity.name_seed || "独特名字",
      identity.hair || "独特发型",
      identity.style || "旅行风格",
    ].join(" · ");
    const accessoryLabel = (accessories || []).map((item) => item.name).filter(Boolean).join("、");
    if (elements.identityMeta) {
      elements.identityMeta.textContent = `身份已锁定 · ${identityLabel}${accessoryLabel ? ` · ${accessoryLabel}` : ""}`;
      elements.identityMeta.hidden = false;
    }
    if (identity.signature) elements.companion.dataset.identitySignature = identity.signature;
    elements.companion.setAttribute("data-identity-label", identityLabel);
    elements.companion.setAttribute("aria-description", identityLabel);
    elements.companion.setAttribute("aria-label", `${identityLabel}的旅行女孩`);
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
    elements.latestPhoto.removeAttribute("src");
    elements.companion.classList.remove("is-identity-pending");
    settlePhotoPending();
    rememberPhoto(travelId, "");
    photoStatus(travelId ? "正在准备她的第一张照片，完成后会自动显示。" : "登录后，开启她的旅途。");
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
    photoStatus("正在加载她的照片…");
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
  const dialog = document.querySelector("#travelDialog");
  const dialogEyebrow = document.querySelector("#dialogEyebrow");
  const dialogTitle = document.querySelector("#dialogTitle");
  const dialogBody = document.querySelector("#dialogBody");
  const dialogPrimary = document.querySelector("#dialogPrimary");
  const messagePreview = document.querySelector("#travelMessagePreview");
  const presenceTitle = document.querySelector("#travelPresenceTitle");
  const presenceMeta = document.querySelector("#travelPresenceMeta");
  const todayPanel = document.querySelector("#travelTodayPanel");
  const journalPanel = document.querySelector("#travelJournalPanel");
  const atlasPanel = document.querySelector("#travelAtlasPanel");
  const journalGrid = document.querySelector("#travelJournalGrid");
  const atlasGrid = document.querySelector("#travelAtlasGrid");
  let travelState = null;
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

  function openDialog(title, body, eyebrow = "旅行女孩", mode = "info") {
    dialogMode = mode;
    dialogEyebrow.textContent = eyebrow;
    dialogTitle.textContent = title;
    dialogBody.textContent = body;
    dialogPrimary.textContent = mode === "adopt" ? "确认领养" : "知道了";
    if (typeof dialog.showModal === "function" && !dialog.open) dialog.showModal();
    else TravelScene.showToast(body);
  }

  function closeDialog() {
    if (dialog.open) dialog.close();
    dialogMode = "info";
  }

  function applyTravelState(nextState) {
    travelState = nextState;
    if (!nextState.girl) return;
    TravelScene.setIdentity(nextState.girl.identity, nextState.girl.initial_accessories);
    const presence = Object.prototype.hasOwnProperty.call(nextState, "presence")
      ? nextState.presence
      : derivePresence(nextState.girl);
    const trip = nextState.girl.current_trip;
    TravelScene.setPresence(presence, trip);
    const travelId = `${nextState.girl.id}:${VISUAL_VERSION}`;
    const scene = nextState.scene || nextState.girl.scene || trip?.scene;
    TravelScene.setScene(scene, presence, trip);
    const messages = Array.isArray(nextState.messages) ? nextState.messages : null;
    // Messages arrive newest first, so the head of this list is the latest photo and the
    // rest only stand in when a link has expired.
    const photos = [...new Set([
      ...(messages || []).filter((message) => message.payload?.visual_version === VISUAL_VERSION).map((message) => message.payload?.postcard_url),
      scene?.photo_url,
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
    const accessories = (preview.initial_accessories || []).map((item) => item.name).join("、");
    return `她叫${identity.name_seed || "旅行女孩"}。${identity.appearance_summary || "她有属于自己的面容、气质和审美。"} 随身小物：${accessories || "专属配饰"}。确认后五官和身材固定，穿搭、配饰和动作会随旅途变化。`;
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
    const label = message.kind === "travel" ? "" : MESSAGE_LABELS[message.kind] || "旅行女孩记录";
    const title = (payload.title || "").trim();
    return {
      label,
      title: title && title !== label && !REDUNDANT_TITLES.has(title) ? title : "",
      text: payload.text || payload.opening || payload.diary?.zh || "旅行女孩把这段生活收进了日志。",
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
      messagePreview.innerHTML = `<div class="travel-empty">${messages.length ? "更早的旅迹还在路上。" : "旅行女孩正在慢慢生活，第一段旅迹还在路上。"}</div>`;
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
      journalGrid.innerHTML = '<div class="travel-empty">这一类日志还没有记录。旅行女孩正在慢慢生活。</div>';
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
    const identity = travelState?.girl?.identity || {};
    const identityText = [identity.name_seed, identity.hair, identity.style, identity.silhouette].filter(Boolean).join(" · ") || "旅行女孩的固定形象";
    const signature = identity.signature ? `<small>身份签名 · ${escapeHtml(identity.signature)}</small>` : "";
    atlasGrid.innerHTML = `<article class="travel-atlas-card travel-atlas-card--identity"><strong>固定身份</strong><p>${escapeHtml(identityText)}</p>${signature}</article>
      <article class="travel-atlas-card"><strong>成长配饰 · ${accessories.length}</strong><p>${accessories.length ? accessories.map((item) => escapeHtml(item.name || item.id)).join(" · ") : "旅行女孩还在积攒自己的旅途"}</p></article>
      <article class="travel-atlas-card"><strong>世界记忆 · ${memories.length}</strong><p class="travel-atlas-memory-list">${memoryText || "第一封信还在路上"}</p></article>`;
  }

  let preparingAdoption = false;
  async function showAdoptionPreview(existingDraft = null) {
    if (preparingAdoption) return;
    if (authState !== "authenticated") {
      openDialog("登录后领养", "登录账户后，才能把这位旅行女孩的生活保存下来。", "旅行女孩");
      return;
    }
    preparingAdoption = true;
    try {
      adoptionPreview = existingDraft;
      if (!adoptionPreview) {
        openDialog("正在准备你们的相遇", "她的模样、性格和故事正在慢慢成形，请稍候。", "新朋友");
        const deadline = Date.now() + 210000;
        while (Date.now() < deadline) {
          let draft;
          try {
            draft = await api("/api/travel/adoption/preview", { method: "POST" });
          } catch (error) {
            if (error.status !== 429) throw error;
          }
          if (draft?.draft_id && draft.identity) {
            adoptionPreview = draft;
            break;
          }
          await new Promise((resolve) => window.setTimeout(resolve, 3000));
        }
        if (!adoptionPreview) throw new Error("她还在准备中，稍后再来看看吧。");
      }
      openDialog("确认领养你的旅行女孩", identityText(adoptionPreview), "第一次相遇", "adopt");
    } catch (error) {
      openDialog("暂时不能领养", error.message || "请稍后再试。", "旅行女孩");
    } finally {
      preparingAdoption = false;
    }
  }

  async function confirmAdoption() {
    if (!adoptionPreview?.draft_id) return;
    dialogPrimary.disabled = true;
    try {
      const data = await api("/api/travel/adoption/confirm", {
        method: "POST",
        body: JSON.stringify({ draft_id: adoptionPreview.draft_id }),
      });
      applyTravelState({ adopted: true, draft: null, girl: data });
      adoptionPreview = null;
      closeDialog();
      TravelScene.react("旅行女孩来到你的旅居空间了。", "我会记住我们的第一次相遇。 ");
      await loadHome();
    } catch (error) {
      if (error.status === 409) {
        adoptionPreview = null;
        await loadTravelState();
        return;
      }
      openDialog("领养还没有完成", error.message || "请重新生成一份身份预览。", "旅行女孩", "info");
    } finally {
      dialogPrimary.disabled = false;
    }
  }

  async function loadTravelState() {
    try {
      const data = await api("/api/travel/state");
      authState = "authenticated";
      applyTravelState(data);
      if (!data.adopted) {
        await showAdoptionPreview(data.draft);
        TravelScene.revealPreview();
      }
      else await loadHome();
    } catch (error) {
      if (error.status === 401 || error.status === 403) {
        authState = "anonymous";
        travelState = { adopted: false, girl: null, draft: null };
        TravelScene.revealPreview();
      } else {
        TravelScene.revealPreview();
        TravelScene.showToast(error.message || "旅行女孩状态暂时无法读取");
      }
    }
  }

  async function loadHome() {
    if (authState !== "authenticated" || !travelState?.adopted) return;
    try {
      // /api/travel/home already excludes retired guide messages, so nothing is filtered here.
      const home = await api("/api/travel/home");
      applyTravelState({ ...home, adopted: true });
      renderMessages(home.messages || []);
    } catch (error) {
      TravelScene.showToast(error.message || "旅居空间状态暂时无法读取");
    }
  }

  function bind() {
    document.querySelector("#travelPhotoRetry")?.addEventListener("click", () => loadHome());
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
          await showAdoptionPreview(travelState?.draft);
          return;
        }
        todayPanel.hidden = item.hash !== "#home";
        journalPanel.hidden = item.hash !== "#journal";
        atlasPanel.hidden = item.hash !== "#atlas";
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
    await loadTravelState();
    window.setInterval(() => {
      if (document.visibilityState === "visible" && authState === "authenticated" && travelState?.adopted) loadHome();
    }, 15_000);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible" && authState === "authenticated" && travelState?.adopted) loadHome();
    });
  }

  return Object.freeze({ init });
})();

document.addEventListener("DOMContentLoaded", () => TravelShell.init());
