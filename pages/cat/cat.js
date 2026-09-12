"use strict";

/*
 * Travel Cat's first page deliberately keeps three boundaries visible:
 *
 * - NewsBridge reads the existing public NEWS feed and converts it into a small,
 *   game-friendly letter. It never owns news data.
 * - CatScene owns only presentation and local interaction feedback.
 * - CatShell wires the two together. Future account state, trip jobs and image
 *   generation can replace one boundary without changing the other two.
 */

const DEMO_LETTER = {
  newsItemId: null,
  title: "京都的春风捎来一封信",
  summary: "猫咪从窗台上抬起头，听见了一个适合出发的季节。把小围巾放进行李箱，它会带一张照片回来。",
  source: "旅行灵感 · NEWS 图片新闻",
  image: null,
  url: "#",
};
const NEWS_TIMEOUT_MS = 6000;

const NewsBridge = Object.freeze({
  async todayLetter() {
    const controller = new AbortController();
    const timeout = window.setTimeout(() => controller.abort(), NEWS_TIMEOUT_MS);
    try {
      // Read the public digest as a source catalogue. The future TravelEvent
      // service will apply the same selection server-side and return a stable
      // event DTO; this prototype keeps the bridge deliberately read-only.
      const response = await fetch("/digest?limit=30", {
        credentials: "same-origin",
        headers: { Accept: "application/json" },
        signal: controller.signal,
      });
      if (!response.ok) throw new Error(`NEWS ${response.status}`);
      const payload = await response.json();
      const candidates = payload.topics?.flatMap((topic) => (topic.items || []).map((item) => ({
        ...item,
        topicName: topic.name,
      }))) || [];
      const item = candidates
        .filter((candidate) => candidate.title && candidate.summary)
        .sort((left, right) => {
          const imageWeight = Number(Boolean(right.image)) - Number(Boolean(left.image));
          return imageWeight || (right.importance || 0) - (left.importance || 0);
        })[0];
      if (!item) return DEMO_LETTER;
      return {
        newsItemId: item.id || null,
        title: item.title || DEMO_LETTER.title,
        summary: item.summary || DEMO_LETTER.summary,
        source: [item.source, item.topicName, "NEWS"].filter(Boolean).join(" · "),
        image: item.image?.url || null,
        url: item.url || "#",
      };
    } catch {
      // The game shell remains useful while NEWS is cold or unavailable.
      return DEMO_LETTER;
    } finally {
      window.clearTimeout(timeout);
    }
  },
});

const CatScene = (() => {
  const elements = {
    stage: document.querySelector(".cat-stage"),
    companion: document.querySelector("#catCompanion"),
    thought: document.querySelector("#catThought"),
    caption: document.querySelector("#catSceneCaption"),
    letterTitle: document.querySelector("#letterTitle"),
    letterSummary: document.querySelector("#letterSummary"),
    letterSource: document.querySelector("#letterSource"),
    letterPhoto: document.querySelector("#letterPhoto"),
    toast: document.querySelector("#catToast"),
  };
  let toastTimer;

  function setLetter(letter) {
    elements.letterTitle.textContent = letter.title;
    elements.letterSummary.textContent = letter.summary;
    elements.letterSource.textContent = letter.source;
    const image = elements.letterPhoto.querySelector("img");
    image.onerror = () => {
      elements.letterPhoto.hidden = true;
      image.removeAttribute("src");
    };
    if (letter.image) {
      image.src = letter.image;
      image.alt = letter.title;
      elements.letterPhoto.hidden = false;
    } else {
      image.removeAttribute("src");
      elements.letterPhoto.hidden = true;
    }
    elements.letterSource.dataset.url = letter.url;
  }

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

  function setIdentity(identity, accessories) {
    if (!identity) return;
    ["coat", "eye_color", "marking"].forEach((key) => {
      const value = identity[key];
      const datasetKey = key === "eye_color" ? "eye" : key;
      if (value) elements.companion.dataset[datasetKey] = value;
      else delete elements.companion.dataset[datasetKey];
    });
    const accent = (accessories || []).find((item) => item.id && item.id !== "sage_scarf");
    if (accent) elements.companion.dataset.accent = accent.id;
    else delete elements.companion.dataset.accent;
    const coatNames = { orange_white: "橘白", cream_tabby: "奶油虎斑", silver_tabby: "银灰虎斑", black_white: "黑白" };
    const eyeNames = { amber: "琥珀色眼睛", green: "绿色眼睛", blue: "蓝色眼睛", hazel: "榛色眼睛" };
    const markingNames = { star: "星形额纹", stripe: "条纹额纹", cloud: "云朵额纹", heart: "心形额纹" };
    const identityLabel = [
      coatNames[identity.coat] || "独特毛色",
      eyeNames[identity.eye_color] || "独特眼睛",
      markingNames[identity.marking] || "特别印记",
    ].join("、");
    if (identity.signature) elements.companion.dataset.identitySignature = identity.signature;
    elements.companion.setAttribute("data-identity-label", identityLabel);
    elements.companion.setAttribute("aria-description", identityLabel);
    elements.companion.setAttribute("aria-label", `一只${identityLabel}的旅行猫咪`);
  }

  function setPresence(presence, trip) {
    elements.stage.dataset.presence = presence || "home_awake";
    elements.companion.dataset.presence = presence || "home_awake";
    const labels = {
      home_awake: "猫咪正在窗边休息",
      home_sleeping: "猫咪已经睡着了",
      away: "猫咪正在旅途中",
      returning_soon: "猫咪快要回来了",
      returned_unread: "猫咪带着旅程来信回来了",
      vigil: "猫咪在灯下安静地读报",
    };
    const captions = {
      home_awake: "在家 · 正在整理今天的世界",
      home_sleeping: "睡着了 · 房间替它守着夜",
      away: "在路上 · 房间里留着一张便条",
      returning_soon: "快回来了 · 门口的灯已经亮起",
      returned_unread: "回来了 · 有一封信和一个包裹",
      vigil: "今晚在家 · 点一盏灯陪世界安静下来",
    };
    elements.caption.textContent = captions[presence] || captions.home_awake;
    elements.thought.textContent = labels[presence] || labels.home_awake;
    const presenceLabel = labels[presence] || labels.home_awake;
    const identityLabel = elements.companion.dataset.identityLabel;
    elements.companion.setAttribute("aria-label", identityLabel ? `${presenceLabel}；${identityLabel}` : presenceLabel);
    elements.stage.setAttribute("aria-label", labels[presence] || labels.home_awake);
    if (trip?.note?.zh && (presence === "away" || presence === "returning_soon")) elements.thought.textContent = trip.note.zh;
  }

  return Object.freeze({ setLetter, setIdentity, setPresence, react, showToast });
})();

const CatShell = (() => {
  const dialog = document.querySelector("#catDialog");
  const dialogEyebrow = document.querySelector("#dialogEyebrow");
  const dialogTitle = document.querySelector("#dialogTitle");
  const dialogBody = document.querySelector("#dialogBody");
  const dialogNameField = document.querySelector("#dialogNameField");
  const dialogName = document.querySelector("#dialogName");
  const dialogPrimary = document.querySelector("#dialogPrimary");
  const catPortal = document.querySelector(".cat-portal");
  const catName = document.querySelector("#catName");
  const unreadCount = document.querySelector("#catUnreadCount");
  const messagePreview = document.querySelector("#catMessagePreview");
  const guideContent = document.querySelector("#catGuideContent");
  const todayPanel = document.querySelector("#catTodayPanel");
  const journalPanel = document.querySelector("#catJournalPanel");
  const atlasPanel = document.querySelector("#catAtlasPanel");
  const journalGrid = document.querySelector("#catJournalGrid");
  const atlasGrid = document.querySelector("#catAtlasGrid");
  let currentLetter = DEMO_LETTER;
  let catState = null;
  let homeState = null;
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
    dialogNameField.hidden = mode !== "rename" && mode !== "adopt";
    dialogPrimary.textContent = mode === "rename" ? "保存名字" : mode === "adopt" ? "确认领养" : "知道了";
    if (typeof dialog.showModal === "function" && !dialog.open) dialog.showModal();
    else CatScene.showToast(body);
  }

  function closeDialog() {
    if (dialog.open) dialog.close();
    dialogMode = "info";
    dialogNameField.hidden = true;
  }

  function applyCatState(nextState) {
    catState = nextState;
    const name = nextState.cat?.name;
    if (name) catName.textContent = name;
    if (typeof nextState.unread_count === "number") {
      unreadCount.textContent = String(nextState.unread_count);
      unreadCount.hidden = nextState.unread_count < 1;
      const inbox = document.querySelector("#catInbox");
      inbox?.setAttribute("aria-label", nextState.unread_count
        ? `猫咪信箱，有 ${nextState.unread_count} 条未读消息`
        : "打开猫咪信箱");
    }
    if (nextState.cat) {
      CatScene.setIdentity(nextState.cat.identity, nextState.cat.initial_accessories);
      const presence = Object.prototype.hasOwnProperty.call(nextState, "presence")
        ? nextState.presence
        : derivePresence(nextState.cat);
      CatScene.setPresence(presence, nextState.cat.current_trip);
    }
  }

  function derivePresence(cat) {
    const status = cat?.current_trip?.status;
    if (status === "returned_unread") return "returned_unread";
    if (status === "away" || status === "settling") return "away";
    const hour = new Date().getHours();
    return hour < 6 || hour >= 23 ? "home_sleeping" : "home_awake";
  }

  function identityText(preview) {
    const identity = preview.identity || {};
    const accessories = (preview.initial_accessories || []).map((item) => item.name).join("、");
    const coatNames = { orange_white: "橘白", cream_tabby: "奶油虎斑", silver_tabby: "银灰虎斑", black_white: "黑白" };
    const eyeNames = { amber: "琥珀色", green: "绿色", blue: "蓝色", hazel: "榛色" };
    return `固定身份：${coatNames[identity.coat] || "独特毛色"}，${eyeNames[identity.eye_color] || "独特眼睛"}，${identity.marking || "特别印记"}。初始配饰：${accessories || "专属配饰"}。确认后身份和初始配饰不会改变，名字以后仍可修改。`;
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

  function messageView(message) {
    const payload = message.payload || {};
    const labels = { welcome: "第一次相遇", guide: "猫咪领读", travel: "旅程动态", postcard: "旅程来信", growth: "成长记录", encounter: "共同世界" };
    const title = payload.title || labels[message.kind] || "猫咪的一条记录";
    const text = payload.text || payload.opening || payload.diary?.zh || "猫咪把这段生活收进了日志。";
    return { labels, title, text, payload };
  }

  function renderMessages(messages) {
    if (!messages.length) {
      messagePreview.innerHTML = '<div class="cat-empty">猫咪还在整理第一封信。它会自己把世界带回来。</div>';
      return;
    }
    messagePreview.innerHTML = messages.map((message) => {
      const view = messageView(message);
      const unread = message.read_at ? "" : " is-unread";
      return `<article class="cat-message-card${unread}" data-message-id="${escapeHtml(message.id)}" role="button" tabindex="0">
        <div class="cat-message-card__kind">${escapeHtml(view.labels[message.kind] || "猫咪记录")}</div>
        <h3>${escapeHtml(view.title)}</h3>
        <p>${escapeHtml(view.text)}</p>
        <time datetime="${escapeHtml(message.created_at)}">${escapeHtml(formatDate(message.created_at))}</time>
      </article>`;
    }).join("");
    messagePreview.querySelectorAll("[data-message-id]").forEach((card) => {
      card.addEventListener("click", () => readMessage(card.dataset.messageId));
      card.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          readMessage(card.dataset.messageId);
        }
      });
    });
  }

  function renderJournal(items, filter = "all") {
    const visible = filter === "all" ? items : items.filter((item) => item.kind === filter);
    if (!visible.length) {
      journalGrid.innerHTML = '<div class="cat-empty">这一类日志还没有记录。猫咪正在慢慢生活。</div>';
      return;
    }
    journalGrid.innerHTML = visible.map((message) => {
      const view = messageView(message);
      const photo = view.payload.postcard_url ? `<div class="cat-journal-card__photo"><img src="${escapeHtml(view.payload.postcard_url)}" alt="${escapeHtml(view.title)}" loading="lazy"></div>` : "";
      const guide = message.kind === "guide" ? `<p>${escapeHtml(view.payload.personality_phrase || view.payload.reflection || "猫咪认真看完了今天的世界。")}</p>` : "";
      return `<article class="cat-journal-card ${message.kind === "postcard" ? "cat-journal-card--postcard" : ""}">
        ${photo}<div class="cat-journal-card__kind">${escapeHtml(view.labels[message.kind] || "猫咪记录")}</div>
        <h3>${escapeHtml(view.title)}</h3><p>${escapeHtml(view.text)}</p>${guide}
        <time datetime="${escapeHtml(message.created_at)}">${escapeHtml(formatDate(message.created_at))}</time>
      </article>`;
    }).join("");
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
      <article class="cat-atlas-card"><strong>成长配饰 · ${accessories.length}</strong><p>${accessories.length ? accessories.map((item) => escapeHtml(item.accessory_id)).join(" · ") : "猫咪还在积攒自己的旅途"}</p></article>
      <article class="cat-atlas-card"><strong>世界记忆 · ${memories.length}</strong><p class="cat-atlas-memory-list">${memoryText || "第一封信还在路上"}</p></article>`;
  }

  async function readMessage(messageId) {
    try {
      const data = await api(`/api/cat/inbox/${messageId}/read`, { method: "POST" });
      unreadCount.textContent = String(data.unread_count || 0);
      unreadCount.hidden = !data.unread_count;
      document.querySelector(`[data-message-id="${CSS.escape(messageId)}"]`)?.classList.remove("is-unread");
    } catch (error) {
      CatScene.showToast(error.message || "信件状态暂时没有保存");
    }
  }

  async function showAdoptionPreview(existingDraft = null) {
    if (authState !== "authenticated") {
      openDialog("登录后领养", "登录账户后，才能把这只猫咪的身份保存下来。访客仍可以先看看今天的世界来信。", "旅行猫咪");
      return;
    }
    try {
      adoptionPreview = existingDraft || await api("/api/cat/adoption/preview", { method: "POST" });
      dialogName.value = "小满";
      openDialog("确认领养你的猫咪", identityText(adoptionPreview), "第一次相遇", "adopt");
      dialogName.focus();
    } catch (error) {
      openDialog("暂时不能领养", error.message || "请稍后再试。", "旅行猫咪");
    }
  }

  async function confirmAdoption() {
    if (!adoptionPreview?.draft_id) return;
    const name = dialogName.value.trim();
    dialogPrimary.disabled = true;
    try {
      const data = await api("/api/cat/adoption/confirm", {
        method: "POST",
        body: JSON.stringify({ draft_id: adoptionPreview.draft_id, name }),
      });
      applyCatState({ adopted: true, draft: null, cat: data });
      adoptionPreview = null;
      closeDialog();
      CatScene.react(`${data.name}来到你的猫窝了。`, "我会记住我们的第一次相遇。 ");
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

  async function renameCat() {
    const name = dialogName.value.trim();
    dialogPrimary.disabled = true;
    try {
      const data = await api("/api/cat/name", { method: "PATCH", body: JSON.stringify({ name }) });
      applyCatState({ ...catState, adopted: true, cat: data });
      closeDialog();
      CatScene.react(`${data.name}，这个名字很好听。`, "名字变了，还是同一只我。 ");
    } catch (error) {
      openDialog("名字没有保存", error.message || "请稍后再试。", "猫咪设置", "rename");
    } finally {
      dialogPrimary.disabled = false;
    }
  }

  async function loadCatState() {
    try {
      const data = await api("/api/cat/state");
      authState = "authenticated";
      applyCatState(data);
      if (!data.adopted) await showAdoptionPreview(data.draft);
      else await loadHome();
    } catch (error) {
      if (error.status === 401 || error.status === 403) {
        authState = "anonymous";
        catState = { adopted: false, cat: null, draft: null };
      } else {
        CatScene.showToast(error.message || "猫咪状态暂时无法读取");
      }
    }
  }

  async function loadHome() {
    if (authState !== "authenticated" || !catState?.adopted) return;
    try {
      homeState = await api("/api/cat/home");
      applyCatState({ ...homeState, adopted: true });
      renderMessages(homeState.messages || []);
      await loadLetter();
    } catch (error) {
      CatScene.showToast(error.message || "猫窝状态暂时无法读取");
    }
  }

  async function onAction(action) {
    if (action !== "travel" && catState?.adopted && authState === "authenticated") {
      const messages = {
        feed: ["吃了一口猫粮，尾巴变得更有精神了。", "今天的猫粮有一点旅行的味道。"],
        water: ["喝了一碗亮晶晶的水。", "喝饱了，出发前还要检查围巾。"],
        toy: ["小铃铛在脚边滚了一圈。", "这个玩具要带上，万一遇见一只友好的猫呢？"],
      };
      try {
        const data = await api("/api/cat/actions", { method: "POST", body: JSON.stringify({ action }) });
        applyCatState({ ...catState, cat: data.cat });
        const [message, thought] = messages[action] || messages.toy;
        CatScene.react(data.applied ? message : "今天已经做过这件事了。", thought);
      } catch (error) {
        CatScene.showToast(error.message || "动作暂时没有记录");
      }
      return;
    }
    if (!catState?.adopted) {
      await showAdoptionPreview(catState?.draft);
      return;
    }
    const messages = {
      travel: ["先整理好行囊，下一步就可以出发了。", "世界这么大，今天去哪里看看？"],
    };
    const [message, thought] = messages[action] || messages.travel;
    CatScene.react(message, thought);
  }

  async function loadLetter() {
    currentLetter = await NewsBridge.todayLetter();
    const trip = catState?.cat?.current_trip;
    if (trip?.news_snapshot) {
      const snapshot = trip.news_snapshot;
      currentLetter = {
        newsItemId: snapshot.id || trip.news_item_id,
        title: trip.status === "returned_unread" ? "猫咪从旅途中寄回一封信" : "猫咪正在路上",
        summary: trip.status === "returned_unread" ? (trip.diary?.zh || "猫咪回来了，把一段世界带到你的猫窝。") : (trip.note?.zh || "猫咪自己整理好行囊，正在把这条消息看得更近一些。"),
        source: [snapshot.source, "猫咪旅迹"].filter(Boolean).join(" · "),
        image: trip.postcard_url || snapshot.image?.url || null,
        url: snapshot.url || "#",
      };
    } else if (trip?.postcard_url) {
      currentLetter = { ...currentLetter, image: trip.postcard_url, source: `${currentLetter.source} · 旅行猫咪明信片` };
    }
    CatScene.setLetter(currentLetter);
    if (currentLetter.image || currentLetter.url !== "#") catPortal?.classList.add("has-news");
  }

  function guideText(data) {
    const cards = (data.items || []).map((item, index) => {
      const title = item.title || `第 ${index + 1} 条新闻`;
      const culture = item.culture_context ? `网络语境：${item.culture_context}\n` : "";
      return `${index + 1}. ${title}\n事实：${item.fact || item.summary || "猫咪正在整理事实。"}\n猫咪的看法：${item.opinion || "我还在认真想这件事。"}\n继续了解：${item.guidance || "可以先看看来源，再和我一起观察后续。"}\n${culture}${item.playful_line || "尾巴同意先把证据放整齐。"}`;
    });
    return [data.opening, ...cards, data.personality_phrase, data.reflection].filter(Boolean).join("\n\n");
  }

  async function showGuide() {
    if (authState !== "authenticated" || !catState?.adopted) {
      await showAdoptionPreview(catState?.draft);
      return;
    }
    const button = document.querySelector("#guideButton");
    if (button) {
      button.disabled = true;
      button.textContent = "猫咪正在整理…";
    }
    try {
      let data = await api("/api/cat/guide");
      for (let attempt = 0; data.status === "generating" && attempt < 20; attempt += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, (data.retry_after_seconds || 3) * 1000));
        data = await api("/api/cat/guide");
      }
      if (data.status === "generating") {
        throw new Error("猫咪还在整理，稍后再点一次领读就好了。");
      }
      guideContent.textContent = guideText(data);
      guideContent.hidden = false;
      todayPanel.hidden = false;
      CatScene.react(data.personality_phrase || "我挑了三件今天值得知道的事。", "先听事实，再听我的一点想法。 ");
    } catch (error) {
      openDialog("领读还在准备", error.message || "猫咪正在把今天的世界整理成一封小信。", "猫咪今日领读");
    } finally {
      if (button) {
        button.disabled = false;
        button.textContent = "听猫咪领读";
      }
    }
  }

  function bind() {
    document.querySelectorAll("[data-action]").forEach((button) => {
      button.addEventListener("click", () => onAction(button.dataset.action));
    });
    document.querySelector("#readButton")?.addEventListener("click", async () => {
      if (currentLetter.url && currentLetter.url !== "#") {
        window.open(currentLetter.url, "_blank", "noopener,noreferrer");
      } else {
        openDialog("来信还在路上", "等新闻图片准备好，小满会把完整来源放在这里。", "NEWS");
      }
      if (currentLetter.newsItemId && authState === "authenticated") {
        try {
          const data = await api("/api/cat/reading", {
            method: "POST",
            body: JSON.stringify({ news_item_id: currentLetter.newsItemId, signal: "open" }),
          });
          if (data.cat) applyCatState({ ...catState, cat: data.cat });
        } catch (error) {
          CatScene.showToast(error.message || "阅读记录暂时没有保存");
        }
      }
      const trip = catState?.cat?.current_trip;
      if (trip?.id && trip.status === "returned_unread" && authState === "authenticated") {
        try {
          const data = await api(`/api/cat/trips/${trip.id}/read`, { method: "POST" });
          applyCatState({ ...catState, cat: data });
        } catch (error) {
          CatScene.showToast(error.message || "来信已打开，但阅读状态暂时没有保存");
        }
      }
    });
    document.querySelector("#guideButton")?.addEventListener("click", showGuide);
    document.querySelector("#catSettings")?.addEventListener("click", () => {
      if (!catState?.adopted) {
        showAdoptionPreview(catState?.draft);
        return;
      }
      dialogName.value = catState.cat.name;
      openDialog("修改猫咪名字", "名字可以随时修改，但这仍然是同一只独一无二的猫咪。", "猫咪设置", "rename");
      dialogName.focus();
    });
    document.querySelector("#catInbox")?.addEventListener("click", async () => {
      if (authState !== "authenticated" || !catState?.adopted) {
        await showAdoptionPreview(catState?.draft);
        return;
      }
      try {
        const data = await api("/api/cat/inbox");
        renderMessages(data.items || []);
        todayPanel.hidden = false;
        messagePreview.scrollIntoView({ behavior: "smooth", block: "center" });
      } catch (error) {
        CatScene.showToast(error.message || "信箱暂时打不开");
      }
    });
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
      else if (dialogMode === "rename") renameCat();
      else closeDialog();
    });
    dialogName?.addEventListener("keydown", (event) => {
      if (event.key === "Enter") dialogPrimary.click();
    });
    dialog?.addEventListener("click", (event) => {
      if (event.target === dialog) closeDialog();
    });
  }

  async function init() {
    bind();
    await loadCatState();
    if (authState !== "authenticated" || !catState?.adopted) await loadLetter();
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
