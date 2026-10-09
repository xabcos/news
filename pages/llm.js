"use strict";

PublicPage.page();

const english = document.documentElement.lang === "en";

(() => {
  const toolbar = document.querySelector(".llm-toolbar");
  const catalog = document.querySelector(".llm-catalog");
  if (!toolbar || !catalog) return;
  const vendors = [...catalog.querySelectorAll(".llm-vendor")];
  const search = toolbar.querySelector(".llm-search");
  const filters = [...toolbar.querySelectorAll("[data-region], [data-category]")];
  const empty = document.createElement("p");
  empty.className = "llm-empty";
  empty.hidden = true;
  empty.textContent = english ? "No matching models." : "没有匹配的模型。";
  catalog.append(empty);
  const chosen = { region: "all", category: "all" };

  // Filtering hides rows in place; without JavaScript every provider stays listed.
  function apply() {
    const query = search.value.trim().toLowerCase();
    let shown = 0;
    for (const vendor of vendors) {
      const inRegion = chosen.region === "all" || vendor.dataset.region === chosen.region;
      let visible = 0;
      for (const group of vendor.querySelectorAll(".llm-group")) {
        const inCategory = chosen.category === "all" || group.dataset.category === chosen.category;
        let rows = 0;
        for (const model of group.querySelectorAll(".llm-model")) {
          const match = inRegion && inCategory && (!query || model.dataset.search.includes(query));
          model.hidden = !match;
          if (match) rows += 1;
        }
        group.hidden = rows === 0;
        visible += rows;
      }
      vendor.hidden = visible === 0;
      shown += visible;
    }
    empty.hidden = shown > 0;
    for (const button of filters) {
      const [key, value] = Object.entries(button.dataset)[0];
      button.setAttribute("aria-pressed", String(chosen[key] === value));
    }
  }

  for (const button of filters) {
    button.addEventListener("click", () => {
      const [key, value] = Object.entries(button.dataset)[0];
      chosen[key] = value;
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
      Object.assign(chosen, { region: "all", category: "all" });
      search.value = "";
      apply();
    }
    row.open = true;
    row.scrollIntoView({ block: "start" });
  }
  window.addEventListener("hashchange", reveal);
  reveal();
})();
