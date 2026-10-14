import { $, api, toast, reportError, withBusy } from "./ui.js";

// Writing an article for 分享. A draft saves itself while being written; a public article
// changes only on 「更新」, which sends it back to review unless an administrator wrote it.
const LIMIT = 10 * 1024 * 1024;
const TYPES = ["image/jpeg", "image/png", "image/webp", "image/gif"];
const statusText = { draft: "草稿", pending: "待审", published: "已公开" };
const title = $("#title");
const body = $("#body");
let article = null; // As the server last saved it: id, revision, status and cover.
let author = null; // Who opened the page; a page left open across a change of account stops.
let admin = false;
let dirty = false; // The editor holds what the server doesn't yet.
let saving = null;
let uploads = Promise.resolve(); // Uploads go one after another, and a submit waits for them.
let timer = 0;
let stopped = ""; // Why this page can't save any more; reloading picks up the latest.
let ready = false;
let busy = false; // A submit or an update is on its way, and the editor holds still until it lands.
let pickingCover = false;

void start();

async function start() {
  try {
    const mine = await api("/api/articles");
    if (!mine.author) {
      location.replace("/me/");
      return;
    }
    author = mine.author.id;
    admin = mine.admin;
    const id = new URLSearchParams(location.search).get("id");
    if (id) {
      article = await api(`/api/articles/${encodeURIComponent(id)}`);
      title.value = article.title;
      body.value = article.body;
      showCover();
    }
    $("#writer").classList.remove("hidden");
    ready = true;
    update();
    (article ? body : title).focus();
  } catch (error) {
    if (error.status === 401) location.replace("/me/");
    else reportError(error);
  }
}

function update(note = "") {
  const status = article?.status || "draft";
  const button = $("#publishButton");
  button.textContent = busy ? "处理中…"
    : status === "published" ? "更新" : status === "pending" ? "审核中" : admin ? "发布" : "提交审核";
  button.disabled = !ready || busy || Boolean(stopped) || (status !== "draft" && !dirty);
  $("#previewButton").disabled = !ready || busy;
  $("#saveState").textContent = note || stopped || (dirty
    ? (status === "published" ? "有未更新的修改" : "未保存")
    : article ? `${statusText[status]} · 已保存` : "");
  const notice = status === "pending" ? "审核中。修改会撤回提交，改完需要重新提交。"
    : status === "published" && !admin ? "已公开。点「更新」后要重新审核，审核通过前文章暂不公开。" : "";
  $("#notice").textContent = notice;
  $("#notice").classList.toggle("hidden", !notice);
}

function changed() {
  dirty = true;
  clearTimeout(timer);
  if (article?.status !== "published") timer = setTimeout(() => void save().catch(reportError), 1500);
  update();
}

// A save that matches nothing means the article changed elsewhere or the account did.
function guard(error) {
  if (error.status === 409) stopped = article ? "已在别处修改，请刷新" : error.message;
  if (article && error.status === 404) stopped = "文章不在当前账户下，请刷新";
  return error;
}

function hold(on) {
  busy = on;
  title.readOnly = on;
  body.readOnly = on;
  update();
}

// One save at a time; what changes meanwhile is saved after it.
async function save() {
  while (saving) await saving.catch(() => {});
  if (stopped || (article && !dirty)) return;
  clearTimeout(timer);
  dirty = false;
  const content = { title: title.value, body: body.value };
  saving = (async () => {
    try {
      if (!article) {
        // Written under the account that opened the page, or not at all.
        article = await api("/api/articles", { method: "POST", body: { author, ...content } });
        history.replaceState(null, "", `?id=${article.id}`);
      } else {
        Object.assign(article, await api(`/api/articles/${article.id}`, {
          method: "PATCH", body: { revision: article.revision, cover: article.cover, ...content },
        }));
      }
    } catch (error) {
      dirty = true;
      throw guard(error);
    } finally {
      saving = null;
      update();
    }
  })();
  return saving;
}

// Saves until the server holds all the editor does, uploads included.
async function flush() {
  await uploads;
  clearTimeout(timer);
  while (!stopped && (saving || dirty || !article)) await save();
}

async function publish() {
  if (article?.status === "published") {
    if (!admin && !window.confirm("更新后要重新审核，审核通过前文章暂不公开。确定更新吗？")) return;
    await flush();
    toast(article.status === "published" ? "已更新" : "已提交审核，通过后重新公开");
    return;
  }
  if (!title.value.trim()) {
    title.focus();
    throw new Error("请先写标题");
  }
  if (!body.value.trim()) {
    body.focus();
    throw new Error("请先写正文");
  }
  await flush();
  if (stopped) return;
  try {
    Object.assign(article, await api(`/api/articles/${article.id}/submit`, {
      method: "POST", body: { revision: article.revision },
    }));
  } catch (error) {
    throw guard(error);
  }
  toast(article.status === "published" ? "已发布" : "已提交，审核通过后公开");
}

// The preview opens at once, so the browser allows it, and loads once the latest is saved.
async function preview() {
  if (!article && !title.value.trim() && !body.value.trim()) throw new Error("先写点内容再预览");
  const tab = window.open("about:blank", "_blank");
  try {
    if (article?.status !== "published") await flush();
    if (!article) throw new Error(stopped || "先写点内容再预览");
    if (dirty) toast("预览的是已保存的内容");
    if (tab) {
      tab.opener = null;
      tab.location.href = `/api/articles/${article.id}/preview`;
    }
  } catch (error) {
    tab?.close();
    throw error;
  }
}

function upload(files, cover = false) {
  if (busy) return;
  uploads = uploads.then(() => uploadNow(files, cover));
}

async function uploadNow(files, cover) {
  let pictures = [...files].filter((file) => TYPES.includes(file.type));
  if (!pictures.length) {
    if (files.length) toast("只支持 JPEG、PNG、WebP 和 GIF 图片", "error");
    return;
  }
  if (cover) pictures = pictures.slice(0, 1);
  try {
    if (!article) await save();
    if (!article || stopped) return;
    for (const [index, file] of pictures.entries()) {
      if (file.size > LIMIT) {
        toast(`「${file.name}」超过 10 MB`, "error");
        continue;
      }
      update(`正在上传 ${index + 1}/${pictures.length}…`);
      const image = await send(file);
      if (cover) {
        article.cover = image.id;
        showCover();
        changed();
      } else {
        insert(`\n![](image:${image.id})\n`);
      }
    }
  } catch (error) {
    reportError(error);
  } finally {
    update();
  }
}

async function send(file) {
  let response;
  try {
    response = await fetch(`/api/articles/${article.id}/images`, {
      method: "POST", credentials: "same-origin", cache: "no-store", body: file,
      headers: { Accept: "application/json", "Content-Type": file.type },
    });
  } catch {
    throw new Error("网络连接失败，请稍后重试");
  }
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    const error = new Error(data?.error?.message || `上传失败（${response.status}）`);
    error.status = response.status;
    throw guard(error);
  }
  return data;
}

function insert(text) {
  body.setRangeText(text, body.selectionStart, body.selectionEnd, "end");
  body.focus();
  changed();
}

function showCover() {
  const cover = article?.cover;
  const image = $("#coverImage");
  image.classList.toggle("hidden", !cover);
  $("#removeCover").classList.toggle("hidden", !cover);
  $("#coverButton").textContent = cover ? "换封面" : "添加封面";
  if (cover) image.src = `/api/articles/${article.id}/images/${cover}`;
  else image.removeAttribute("src");
}

for (const field of [title, body]) field.addEventListener("input", changed);
$("#publishButton").addEventListener("click", async () => {
  if (busy) return;
  hold(true);
  try {
    await publish();
  } catch (error) {
    reportError(error);
  } finally {
    hold(false);
  }
});
$("#previewButton").addEventListener("click", () => withBusy($("#previewButton"), null, preview));
$("#coverButton").addEventListener("click", () => {
  if (busy) return;
  pickingCover = true;
  $("#fileInput").click();
});
$("#removeCover").addEventListener("click", () => {
  if (busy) return;
  article.cover = null;
  showCover();
  changed();
});
$("#fileInput").addEventListener("change", () => {
  const files = [...$("#fileInput").files];
  $("#fileInput").value = "";
  upload(files, pickingCover);
  pickingCover = false;
});
$("#toolbar").addEventListener("click", (event) => {
  const control = event.target.closest("button");
  if (!control || busy) return;
  if (control.dataset.action === "image") {
    pickingCover = false;
    $("#fileInput").click();
    return;
  }
  const { before = "", after = "", line, text } = control.dataset;
  const start = body.selectionStart;
  const end = body.selectionEnd;
  body.focus();
  if (line !== undefined) {
    const lineStart = body.value.lastIndexOf("\n", start - 1) + 1;
    body.setRangeText(line, lineStart, lineStart, "preserve");
  } else if (text !== undefined) {
    // A divider goes after the line, with blank lines around it so the line above stays text.
    const lineEnd = body.value.indexOf("\n", end);
    const at = lineEnd < 0 ? body.value.length : lineEnd;
    body.setRangeText(text, at, at, "end");
  } else {
    // The marks go around the selection, which stays selected inside them.
    body.setRangeText(`${before}${body.value.slice(start, end)}${after}`, start, end);
    body.setSelectionRange(start + before.length, end + before.length);
  }
  changed();
});
body.addEventListener("paste", (event) => {
  const files = [...event.clipboardData.files];
  if (!files.length) return;
  event.preventDefault();
  upload(files);
});
body.addEventListener("dragover", (event) => {
  if (event.dataTransfer.types.includes("Files")) event.preventDefault();
});
body.addEventListener("drop", (event) => {
  if (!event.dataTransfer.files.length) return;
  event.preventDefault();
  upload(event.dataTransfer.files);
});
// A draft hidden behind another tab saves right away.
document.addEventListener("visibilitychange", () => {
  if (document.hidden && dirty && article?.status !== "published") void save().catch(reportError);
});
window.addEventListener("beforeunload", (event) => {
  if (dirty || saving) event.preventDefault();
});
