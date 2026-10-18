import { $, node, withBusy, toast, reportError, confirmDelete, formatTime } from "./ui.js";

const statusText = { draft: "草稿", pending: "待审", published: "已公开" };

// 分享 in the workspace: the pen name, the reader's own articles and, for an administrator,
// what waits for a ❤️. Requests share the session's abort signal.
export function createArticles(session) {
  const signal = session.controller.signal;
  let author = null;
  let version = 0;
  // An administrator finds older public articles by day or by words, to copy them again.
  const find = { day: "", q: "" };

  $("#penNameButton").addEventListener("click", () => editName(), { signal });
  $("#writeButton").addEventListener("click", (event) => {
    if (author) return;
    event.preventDefault();
    editName();
    toast("写文章前先设置笔名");
  }, { signal });
  $("#penNameForm").addEventListener("submit", (event) => {
    event.preventDefault();
    void withBusy($("#penNameForm button"), "保存中…", async () => {
      await session.request("/api/author", { method: "PUT", body: { name: $("#penName").value } });
      $("#penNameForm").classList.add("hidden");
      toast("笔名已保存");
      await load();
    });
  }, { signal });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") void load().catch(reportError);
  }, { signal });

  function editName() {
    $("#penName").value = author?.name || "";
    $("#penNameForm").classList.remove("hidden");
    $("#penName").focus();
  }

  async function load() {
    const current = ++version;
    const [mine, review] = await Promise.all([
      session.request("/api/articles"),
      // Only what is filled in: an empty day is no date to the server.
      session.admin ? session.request(`/api/admin/articles?${new URLSearchParams(Object.entries(find).filter(([, value]) => value))}`) : null,
    ]);
    if (current !== version || signal.aborted) return;
    author = mine.author;
    $("#authorLine").textContent = author ? `笔名：${author.name}` : "写文章前先设置笔名，读者只看到笔名";
    $("#penNameButton").textContent = author ? "修改笔名" : "设置笔名";
    renderMine(mine.articles);
    if (review) renderReview(review);
  }

  function renderMine(articles) {
    const message = $("#articlesMessage");
    message.textContent = articles.length ? "" : "还没有文章。写点有趣好玩的东西，审核通过后公开。";
    message.classList.toggle("hidden", Boolean(articles.length));
    $("#articleList").replaceChildren(...articles.map((article) => {
      const row = node("li", "item-row");
      const title = node("a", "", article.title || "无标题");
      title.href = `/me/write.html?id=${encodeURIComponent(article.id)}`;
      const actions = node("div", "item-actions");
      actions.append(link("编辑", title.href), link("预览", `/api/articles/${encodeURIComponent(article.id)}/preview`, true));
      if (article.status === "published") actions.append(link("查看", `/articles/${encodeURIComponent(article.id)}`, true));
      const remove = node("button", "button button-quiet button-small danger-text", "删除");
      remove.type = "button";
      remove.addEventListener("click", () => confirmDelete("删除文章", `确定删除「${article.title || "无标题"}」吗？删除后不能恢复。`, async () => {
        await session.request(`/api/articles/${encodeURIComponent(article.id)}`, { method: "DELETE" });
        toast("文章已删除");
        await load();
      }));
      actions.append(remove);
      row.append(title, node("p", "item-meta muted", `${statusText[article.status]} · 阅读 ${article.views} · 更新于 ${formatTime(article.updated_at, session.timezone)}`), actions);
      return row;
    }));
  }

  // The administrator reads each article in its preview, where the ❤️ and the copies for X
  // Articles are, and edits anyone's, XABC's above all.
  function renderReview({ pending, published }) {
    const lists = $("#reviewLists");
    lists.classList.remove("hidden");
    const section = (heading, articles, empty, extra) => {
      const box = node("div", "review-list");
      box.append(node("h3", "", heading));
      if (!articles.length) box.append(node("p", "muted small", empty));
      const list = node("ul", "item-list");
      list.append(...articles.map((article) => {
        const row = node("li", "item-row");
        const title = node("a", "", article.title || "无标题");
        title.href = `/api/admin/preview/articles/${encodeURIComponent(article.id)}`;
        title.target = "_blank";
        title.rel = "noopener";
        const actions = node("div", "item-actions");
        actions.append(link("编辑", `/me/write.html?id=${encodeURIComponent(article.id)}`));
        row.append(title, node("p", "item-meta muted", `${article.author} · ${extra(article)}`), actions);
        return row;
      }));
      box.append(list);
      return box;
    };
    const finding = Boolean(find.day || find.q);
    const recent = section(finding ? `找到 ${published.length} 篇` : "最近公开", published, finding ? "没有找到。" : "还没有公开的文章。",
      (article) => `${article.film ? "解说" : article.lesson ? "学习" : "分享"} · 阅读 ${article.views} · 公开于 ${formatTime(article.published_at, session.timezone)}`);
    recent.querySelector("h3").after(finder());
    lists.replaceChildren(
      section(`待审（${pending.length}）`, pending, "没有等待审核的文章。", (article) => `更新于 ${formatTime(article.updated_at, session.timezone)}`),
      recent,
    );
  }

  // A day, words in the title or opening, or both; 最近 goes back to the latest.
  function finder() {
    const form = node("form", "review-find inline");
    const day = Object.assign(node("input"), { type: "date", value: find.day });
    day.setAttribute("aria-label", "公开日期");
    const words = Object.assign(node("input"), { type: "search", value: find.q, placeholder: "标题或开头里的词" });
    words.setAttribute("aria-label", "关键词");
    const search = node("button", "button button-quiet button-small", "查找");
    const latest = node("button", "button button-quiet button-small", "最近");
    latest.type = "button";
    const run = () => void load().catch(reportError);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      Object.assign(find, { day: day.value, q: words.value.trim() });
      run();
    });
    day.addEventListener("change", () => form.requestSubmit());
    latest.addEventListener("click", () => {
      Object.assign(find, { day: "", q: "" });
      run();
    });
    form.append(day, words, search, latest);
    return form;
  }

  function link(text, href, external = false) {
    const element = node("a", "button button-quiet button-small", text);
    element.href = href;
    if (external) {
      element.target = "_blank";
      element.rel = "noopener";
    }
    return element;
  }

  return {
    load,
    destroy() {
      version += 1;
      author = null;
      for (const id of ["articleList", "reviewLists"]) $(`#${id}`).replaceChildren();
      $("#reviewLists").classList.add("hidden");
      $("#penNameForm").classList.add("hidden");
      $("#authorLine").textContent = "";
      $("#articlesMessage").classList.add("hidden");
    },
  };
}
