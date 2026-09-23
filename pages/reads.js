"use strict";

PublicPage.init();

const english = document.documentElement.lang === "en";
try { localStorage.setItem("news.lang", english ? "en" : "zh"); } catch { /* Reading works without storage. */ }

const activeTab = document.querySelector('.tabs [aria-current="true"]');
activeTab?.scrollIntoView({ block: "nearest", inline: "center" });

const copy = document.querySelector("#copyRead");
if (copy && navigator.clipboard) {
  copy.hidden = false;
  copy.addEventListener("click", async () => {
    const status = document.querySelector("#copyStatus");
    try {
      await navigator.clipboard.writeText(document.querySelector('link[rel="canonical"]').href);
      status.textContent = english ? "Copied" : "已复制";
    } catch { status.textContent = english ? "Copy the address above" : "可复制浏览器地址"; }
    window.setTimeout(() => { status.textContent = ""; }, 2500);
  });
}
