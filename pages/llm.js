"use strict";

PublicPage.init();

const english = document.documentElement.lang === "en";
try { localStorage.setItem("news.lang", english ? "en" : "zh"); } catch { /* Reading works without storage. */ }
document.querySelector('.tabs [aria-current="true"]')?.scrollIntoView({ block: "nearest", inline: "center" });

(() => {
  const toolbar = document.querySelector(".llm-toolbar");
  const catalog = document.querySelector(".llm-catalog");
  if (!toolbar || !catalog) return;
  const vendors = [...catalog.querySelectorAll(".llm-vendor")];
  const search = toolbar.querySelector(".llm-search");
  const regions = [...toolbar.querySelectorAll("[data-region]")];
  const empty = document.createElement("p");
  empty.className = "llm-empty";
  empty.hidden = true;
  empty.textContent = english ? "No matching models." : "没有匹配的模型。";
  catalog.append(empty);
  let region = "all";

  // Filtering hides rows in place; without JavaScript every provider stays listed.
  function apply() {
    const query = search.value.trim().toLowerCase();
    let shown = 0;
    for (const vendor of vendors) {
      const inRegion = region === "all" || vendor.dataset.region === region;
      let visible = 0;
      for (const model of vendor.querySelectorAll(".llm-model")) {
        const match = inRegion && (!query || model.dataset.search.includes(query));
        model.hidden = !match;
        if (match) visible += 1;
      }
      vendor.hidden = visible === 0;
      shown += visible;
    }
    empty.hidden = shown > 0;
    for (const button of regions) button.setAttribute("aria-pressed", String(button.dataset.region === region));
  }

  for (const button of regions) {
    button.addEventListener("click", () => {
      region = button.dataset.region;
      apply();
    });
  }
  search.addEventListener("input", apply);
  toolbar.hidden = false;

  // An opened row becomes the address, so a model can be linked directly.
  catalog.addEventListener("toggle", (event) => {
    const row = event.target;
    if (!row.matches?.(".llm-model")) return;
    if (row.open) history.replaceState(null, "", `#${encodeURIComponent(row.id)}`);
    else if (decodeURIComponent(location.hash.slice(1)) === row.id) history.replaceState(null, "", location.pathname + location.search);
  }, true);

  function reveal() {
    const id = decodeURIComponent(location.hash.slice(1));
    const row = id ? document.getElementById(id) : null;
    if (!row?.matches(".llm-model")) return;
    if (row.hidden) {
      region = "all";
      search.value = "";
      apply();
    }
    row.open = true;
    row.scrollIntoView({ block: "start" });
  }
  window.addEventListener("hashchange", reveal);
  reveal();
})();
