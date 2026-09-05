export const $ = (selector, root = document) => root.querySelector(selector);

export async function api(path, { body, ...options } = {}) {
  let response;
  try {
    response = await fetch(path, {
      credentials: "same-origin", cache: "no-store", ...options,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      headers: { Accept: "application/json", ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
    });
  } catch (error) {
    if (error.name === "AbortError") throw error;
    throw new Error("网络连接失败，请稍后重试");
  }
  const data = response.status === 204 ? null : await response.json().catch(() => null);
  options.signal?.throwIfAborted();
  if (!response.ok) {
    const error = new Error(data?.error?.message || `请求失败（${response.status}）`);
    error.status = response.status;
    throw error;
  }
  return data;
}

export function node(tag, className = "", text = "") {
  const element = document.createElement(tag);
  element.className = className;
  element.textContent = text;
  return element;
}

export function button(text, action, style = "button-quiet") {
  const element = node("button", `button button-small ${style}`, text);
  element.type = "button";
  element.addEventListener("click", () => withBusy(element, "处理中…", action));
  return element;
}

export async function withBusy(element, label, action) {
  if (element.disabled) return;
  const original = element.textContent;
  element.disabled = true;
  element.setAttribute("aria-busy", "true");
  element.textContent = label ?? original;
  try { await action(); }
  catch (error) { reportError(error); }
  finally {
    element.disabled = false;
    element.removeAttribute("aria-busy");
    element.textContent = original;
  }
}

let toastTimer;
export function toast(message, type = "success") {
  const element = node("div", `toast ${type}`, message);
  element.setAttribute("role", type === "error" ? "alert" : "status");
  const region = $("#toastRegion");
  (Array.from(document.querySelectorAll("dialog[open]")).at(-1) || document.body).append(region);
  region.replaceChildren(element);
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => element.remove(), type === "error" ? 7000 : 4000);
}

export function reportError(error) {
  if (error.name !== "AbortError") toast(error.message, "error");
}

export function openDialog(dialog) {
  if (!dialog.open) dialog.showModal();
  document.body.classList.add("dialog-open");
}

export function closeDialog(dialog) {
  dialog.close();
  document.body.classList.toggle("dialog-open", Boolean($("dialog[open]")));
}

export function bindDialogs() {
  document.querySelectorAll("dialog").forEach((dialog) => {
    const busy = () => Boolean($("[aria-busy='true']", dialog));
    const outside = (event) => {
      const rect = dialog.getBoundingClientRect();
      return event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom);
    };
    let pressedOutside = false;
    dialog.addEventListener("pointerdown", (event) => { pressedOutside = outside(event); });
    dialog.addEventListener("click", (event) => {
      if (pressedOutside && outside(event) && !busy()) closeDialog(dialog);
      pressedOutside = false;
    });
    dialog.addEventListener("cancel", (event) => { if (busy()) event.preventDefault(); });
    dialog.addEventListener("close", () => {
      document.body.classList.toggle("dialog-open", Boolean($("dialog[open]")));
      const region = $("#toastRegion");
      if (region.closest("dialog") === dialog) {
        (Array.from(document.querySelectorAll("dialog[open]")).at(-1) || document.body).append(region);
      }
    });
    dialog.querySelectorAll("[data-close-dialog]").forEach((control) => control.addEventListener("click", () => {
      if (!busy()) closeDialog(dialog);
    }));
  });
}

export function confirmDelete(title, message, action) {
  $("#confirmTitle").textContent = title;
  $("#confirmMessage").textContent = message;
  const dialog = $("#confirmDialog");
  const control = $("#confirmActionButton");
  control.onclick = () => withBusy(control, "删除中…", async () => { await action(); closeDialog(dialog); });
  openDialog(dialog);
}

export function safeUrl(value, sameOrigin = false) {
  if (!value) return "";
  try {
    const url = new URL(value, location.href);
    return ["http:", "https:"].includes(url.protocol) && (!sameOrigin || url.origin === location.origin) ? url.href : "";
  } catch { return ""; }
}

export function formatTime(value, timezone) {
  return value ? new Date(value).toLocaleString("zh-CN", {
    timeZone: timezone, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false,
  }) : "—";
}

export const money = (fen) => `¥${(fen / 100).toFixed(2).replace(/\.00$/, "")}`;
export const cost = (ticks) => `$${(ticks * 1e-10).toFixed(3)}`;
