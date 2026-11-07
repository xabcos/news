import { $, node, withBusy, toast, reportError, confirmDelete, formatTime } from "./ui.js";

const statusText = { draft: "草稿", pending: "待审", published: "已公开" };
// Which article it is: a reader's 分享 or one of XABC's lines.
const kind = (article) => article.film ? "解说" : article.person ? "时代" : article.event ? "追踪" : article.product ? "评测" : "分享";

// 分享 in the workspace: the pen name, the reader's own articles and, for an administrator,
// what XABC writes next and the articles to review. Requests share the session's abort signal.
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
  // What an administrator asks XABC to write joins the picked line's list, written before its
  // own articles: something 分享 looks into, or a person or a company 时代 writes about.
  const lines = {
    share: { path: "/api/admin/research", key: "brief", hint: "想了解什么，XABC 全网调研后写成文章，比如：某个新模型怎么调用，以官方文档为准", told: () => "已排进分享" },
    people: { path: "/api/admin/people", key: "name", hint: "人物或企业的名字", told: (name) => `「${name}」已排进时代` },
  };
  const ask = $("#askForm");
  const text = $("#askText");
  const line = () => lines[ask.elements.line.value];
  const hint = () => { text.placeholder = line().hint; };
  hint();
  ask.classList.toggle("hidden", !session.admin);
  ask.addEventListener("change", hint, { signal });
  ask.addEventListener("submit", (event) => {
    event.preventDefault();
    const { path, key, told } = line();
    void withBusy($("#askForm button"), "提交中…", async () => {
      const value = text.value.trim();
      const { ahead } = await session.request(path, { method: "POST", body: { [key]: value } });
      // Whatever was typed meanwhile, for either line, stays.
      if (text.value.trim() === value) text.value = "";
      toast(`${told(value)}，${ahead ? `前面还有 ${ahead} 个` : "下一篇就写它"}`);
    });
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
      actions.append(removal(article));
      row.append(title, node("p", "item-meta muted", `${statusText[article.status]} · 阅读 ${article.views} · 更新于 ${formatTime(article.updated_at, session.timezone)}`), actions);
      return row;
    }));
  }

  // The administrator reads each article in its preview, where the ❤️ and the copies for X
  // Articles are, edits anyone's, XABC's above all, and deletes what waits for a ❤️; a public
  // one goes back to wait first. What waits shows only while something does, with why: the AI
  // held it or an administrator took it back.
  function renderReview({ pending, published }) {
    const lists = $("#reviewLists");
    lists.classList.remove("hidden");
    const section = (heading, articles, extra, removable = false) => {
      const box = node("div", "review-list");
      box.append(node("h3", "", heading));
      const list = node("ul", "item-list");
      list.append(...articles.map((article) => {
        const row = node("li", "item-row");
        const title = node("a", "", article.title || "无标题");
        title.href = `/api/admin/preview/articles/${encodeURIComponent(article.id)}`;
        title.target = "_blank";
        title.rel = "noopener";
        const actions = node("div", "item-actions");
        actions.append(link("编辑", `/me/write.html?id=${encodeURIComponent(article.id)}`));
        if (removable) actions.append(removal(article));
        row.append(title, node("p", "item-meta muted", `${article.author} · ${extra(article)}`), actions);
        return row;
      }));
      box.append(list);
      return box;
    };
    const finding = Boolean(find.day || find.q);
    const recent = section(finding ? `找到 ${published.length} 篇` : "最近公开", published,
      (article) => `${kind(article)} · 阅读 ${article.views} · 公开于 ${formatTime(article.published_at, session.timezone)}`);
    recent.querySelector("h3").after(finder(finding));
    if (!published.length) recent.append(node("p", "muted small", finding ? "没有找到。" : "还没有公开的文章。"));
    lists.replaceChildren(
      ...(pending.length ? [section(`待审（${pending.length}）`, pending, (article) => [`更新于 ${formatTime(article.updated_at, session.timezone)}`, article.held].filter(Boolean).join(" · "), true)] : []),
      recent,
    );
  }

  // Deleting can't be undone. An article of XABC's lines waits for a ❤️ only once it was taken
  // back, which keeps its line off the subject; deleted, the line may take the subject up again.
  function removal(article) {
    const line = kind(article);
    const again = line === "分享" ? "" : `留在待审，${line}就不会再写这个选题；删除后可能再写。`;
    const remove = node("button", "button button-quiet button-small danger-text", "删除");
    remove.type = "button";
    remove.addEventListener("click", () => confirmDelete("删除文章", `确定删除「${article.title || "无标题"}」吗？删除后不能恢复。${again}`, async () => {
      await session.request(`/api/articles/${encodeURIComponent(article.id)}`, { method: "DELETE" });
      toast("文章已删除");
      await load();
    }));
    return remove;
  }

  // A day, words in the title or opening, or both, looked for once a day is picked or Enter is
  // pressed in the words, not while an input method composes them; while finding, 最近 goes back
  // to the latest.
  function finder(finding) {
    const form = node("form", "review-find inline");
    const day = Object.assign(node("input"), { type: "date", value: find.day });
    day.setAttribute("aria-label", "公开日期");
    const words = Object.assign(node("input"), { type: "search", value: find.q, placeholder: "标题或开头里的词" });
    words.setAttribute("aria-label", "关键词");
    const run = () => void load().catch(reportError);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      Object.assign(find, { day: day.value, q: words.value.trim() });
      run();
    });
    day.addEventListener("change", () => form.requestSubmit());
    // Some browsers submit the words on Enter themselves, some don't: it is done here, once.
    words.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" || event.isComposing || event.keyCode === 229) return;
      event.preventDefault();
      form.requestSubmit();
    });
    form.append(day, words);
    if (finding) {
      const latest = node("button", "button button-quiet button-small", "最近");
      latest.type = "button";
      latest.addEventListener("click", () => {
        Object.assign(find, { day: "", q: "" });
        run();
      });
      form.append(latest);
    }
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
      $("#askForm").classList.add("hidden");
      $("#authorLine").textContent = "";
      $("#articlesMessage").classList.add("hidden");
    },
  };
}
