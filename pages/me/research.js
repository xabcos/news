import { $, node, button, toast, confirmDelete, formatTime } from "./ui.js";

const statusText = { draft: "不公开", pending: "待审", published: "已公开" };

// 调研 in the workspace: the reader's questions in the queue and the articles written for them,
// or for an administrator everyone's, with who asked, Pro or Free, and what isn't public. Pro
// readers switch whether their own go public; the words are XABC's, which only an administrator
// edits. The panel shows only while it has something, and looks again every minute while a
// question waits.
export function createResearch(session) {
  const signal = session.controller.signal;
  let version = 0;
  let timer = null;

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") void load().catch(() => {});
  }, { signal });

  async function load() {
    const current = ++version;
    clearTimeout(timer);
    const data = await session.request("/api/research");
    if (current !== version || signal.aborted) return;
    const { queue, articles } = data;
    $("#researchPanel").classList.toggle("hidden", !queue.length && !articles.length);
    $("#researchLine").textContent = session.admin
      ? `排队 ${queue.filter((asked) => !asked.failed).length} 篇 · 一篇约 ${data.minutes} 分钟 · Pro 排在 Free 前面`
      : data.limit === 0 ? "调研是 Pro 功能" : `今天还能调研 ${data.left} 次 · ${data.private ? "Pro" : "Free"}`;
    $("#researchList").replaceChildren(...queue.map((asked) => waiting(asked, data)), ...articles.map((article) => written(article, data)));
    if (queue.some((asked) => !asked.failed)) timer = setTimeout(() => void load().catch(() => {}), 60_000);
  }

  // An administrator's lines lead with badges, Pro or Free and what isn't public, and name who
  // asked.
  const badges = (pro, ...more) => session.admin ? [badge(pro ? "Pro" : "Free", pro ? "badge-pro" : "badge-free"), ...more] : [];
  const asker = (item) => session.admin ? [item.author] : [];

  function waiting(asked, data) {
    const row = node("li", "item-row");
    const state = asked.failed ? "没写成，额度已退回"
      : asked.started_at ? `写作中 · 已 ${Math.max(1, Math.round((Date.now() - new Date(asked.started_at)) / 60_000))} 分钟`
        : asked.ahead ? `排队中 · 前面还有 ${asked.ahead} 位 · 约 ${(asked.ahead + 1) * data.minutes} 分钟后写好` : "马上开始";
    const meta = node("p", "item-meta muted");
    meta.append(...badges(asked.pro, ...(asked.public ? [] : [badge("不公开", "badge-private")])),
      [...asker(asked), state, `提交于 ${formatTime(asked.asked_at, session.timezone)}`].join(" · "));
    const actions = node("div", "item-actions");
    if (data.private && asked.mine && !asked.failed) actions.append(publicity(asked.id, asked.public));
    // A reader takes back what waits or clears what failed; an administrator removes any.
    if (session.admin || asked.failed || !asked.started_at) {
      actions.append(button(asked.failed ? "清除" : asked.mine ? "取消" : "删除", async () => {
        await session.request(`/api/research/${encodeURIComponent(asked.id)}`, { method: "DELETE" });
        await load();
      }, "button-quiet danger-text"));
    }
    row.append(node("p", "research-asked", asked.question), meta, actions);
    return row;
  }

  function written(article, data) {
    const row = node("li", "item-row");
    const id = encodeURIComponent(article.id);
    // A public article opens as readers see it, one in review where an administrator gives the ❤️,
    // and the rest in the author's preview, which an administrator reads too.
    const title = Object.assign(node("a", "", article.title || "无标题"), {
      href: article.status === "published" ? `/articles/${id}`
        : session.admin && article.status === "pending" ? `/api/admin/preview/articles/${id}` : `/api/articles/${id}/preview`,
      target: "_blank", rel: "noopener",
    });
    const meta = node("p", "item-meta muted");
    // An administrator sees at a glance what isn't public, and why it waits; a reader, its status.
    const shown = article.status === "published" ? [] : [badge(statusText[article.status], article.status === "draft" ? "badge-private" : "badge-held")];
    meta.append(...badges(article.pro, ...shown),
      [...asker(article), session.admin ? article.held : statusText[article.status], `阅读 ${article.views}`, `写好于 ${formatTime(article.created_at, session.timezone)}`]
        .filter(Boolean).join(" · "));
    const actions = node("div", "item-actions");
    if (data.private && article.mine) actions.append(publicity(article.id, article.status !== "draft"));
    if (session.admin) actions.append(Object.assign(node("a", "button button-quiet button-small", "编辑"), { href: `/me/write.html?id=${id}` }));
    actions.append(button("删除", async () => confirmDelete("删除调研", `确定删除「${article.title || "无标题"}」吗？删除后不能恢复。`, async () => {
      await session.request(`/api/articles/${id}`, { method: "DELETE" });
      toast("调研已删除");
      await load();
    }), "button-quiet danger-text"));
    row.append(title, meta, actions);
    return row;
  }

  function publicity(id, shown) {
    const label = node("label", "public-switch");
    const box = Object.assign(node("input"), { type: "checkbox", checked: shown });
    box.addEventListener("change", async () => {
      box.disabled = true;
      try {
        const { status } = await session.request(`/api/research/${encodeURIComponent(id)}`, { method: "PATCH", body: { public: box.checked } });
        if (status === "pending") toast("已交给 AI 审核，通过后公开");
      } catch (error) {
        toast(error.message, "error");
      }
      await load().catch(() => {});
    });
    label.append(box, "公开");
    return label;
  }

  return {
    load,
    destroy() {
      version += 1;
      clearTimeout(timer);
      $("#researchList").replaceChildren();
      $("#researchLine").textContent = "";
      $("#researchPanel").classList.add("hidden");
    },
  };
}

function badge(text, kind) {
  return node("span", `badge ${kind}`, text);
}
