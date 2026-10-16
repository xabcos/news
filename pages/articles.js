"use strict";

PublicPage.page();

// A read counts once a day in this browser, and the server leaves out the author's own. The
// page may come from a cache, so the count it shows is brought up to date here.
(() => {
  const counter = document.querySelector("[data-article-views]");
  const id = counter?.dataset.articleViews;
  if (!id) return;
  const key = `news.read.${id}`;
  const today = new Date().toDateString();
  try {
    if (localStorage.getItem(key) === today) return;
  } catch { /* Counting works without storage. */ }
  fetch(`/api/articles/${encodeURIComponent(id)}/views`, { method: "POST", credentials: "same-origin" })
    .then((response) => (response.ok ? response.json() : null))
    .then((data) => {
      if (!Number.isSafeInteger(data?.views)) return;
      try { localStorage.setItem(key, today); } catch { /* Counting works without storage. */ }
      counter.querySelector("span").textContent = new Intl.NumberFormat(document.documentElement.lang).format(data.views);
    })
    .catch(() => {});
})();

// An administrator's preview: the ❤️ publishes the revision on screen, and taking it back
// returns the article to review.
(() => {
  const control = document.querySelector("[data-article-review]");
  if (!control) return;
  control.addEventListener("click", async () => {
    control.disabled = true;
    try {
      const response = await fetch(`/api/admin/articles/${encodeURIComponent(control.dataset.articleReview)}`, {
        method: "PATCH",
        credentials: "same-origin",
        headers: { Accept: "application/json", "Content-Type": "application/json" },
        body: JSON.stringify({ published: control.dataset.published !== "true", revision: Number(control.dataset.revision) }),
      });
      if (!response.ok) {
        const data = await response.json().catch(() => null);
        throw new Error(data?.error?.message || `HTTP ${response.status}`);
      }
      location.reload();
    } catch (error) {
      control.disabled = false;
      window.alert(error.message);
    }
  });
})();

// An administrator copies an article to post it on X Articles: the title on its own, and the
// body as formatted text with plain text beside it. X takes pictures only as uploads, so each
// picture leaves a note of where it goes and what its caption says.
(() => {
  const article = document.querySelector(".article");
  const controls = document.querySelectorAll("[data-article-copy]");
  if (!article || !controls.length) return;
  const write = async (text, html) => {
    if (html && typeof ClipboardItem !== "undefined" && navigator.clipboard.write) {
      try {
        await navigator.clipboard.write([new ClipboardItem({
          "text/plain": new Blob([text], { type: "text/plain" }),
          "text/html": new Blob([html], { type: "text/html" }),
        })]);
        return;
      } catch { /* Some browsers only allow plain-text clipboard writes. */ }
    }
    await navigator.clipboard.writeText(text);
  };
  const body = () => {
    const copy = article.querySelector(".read-body").cloneNode(true);
    copy.querySelectorAll("img").forEach((image, index) => {
      // A caption is the emphasis right after a picture, with at most a line break between.
      let caption = image.nextSibling;
      if (caption?.nodeType === Node.TEXT_NODE && !caption.textContent.trim()) caption = caption.nextSibling;
      if (caption?.nodeName !== "EM") caption = null;
      const words = caption?.textContent.trim() || image.alt.trim();
      const note = document.createElement("span");
      note.textContent = `〔配图 ${index + 1}${words ? `：${words}` : ""}〕`;
      caption?.remove();
      image.replaceWith(note);
    });
    const text = [...copy.children]
      .map((block) => (block.matches("ul, ol")
        ? [...block.children].map((item, index) => `${block.matches("ol") ? `${index + 1}.` : "•"} ${item.textContent.trim()}`).join("\n")
        : block.textContent.trim()))
      .filter(Boolean)
      .join("\n\n");
    return { text, html: copy.innerHTML };
  };
  for (const control of controls) {
    const label = control.textContent;
    control.addEventListener("click", async () => {
      try {
        if (control.dataset.articleCopy === "title") {
          await write(article.querySelector(".read-title h1").textContent.trim());
        } else {
          const { text, html } = body();
          await write(text, html);
        }
        control.textContent = document.documentElement.lang === "en" ? "Copied" : "已复制";
        setTimeout(() => { control.textContent = label; }, 1500);
      } catch (error) {
        window.alert(error.message);
      }
    });
  }
})();
