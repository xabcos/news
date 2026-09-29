/* A focused reading view for derived notes; raw collection items stay in the workspace. */
const EquityNotes = (() => {
  let market = "all";
  const labels = {
    zh: { title: "股市观察", empty: "股市研判笔记将在生成后显示。", all: "全部", cn: "A 股", us: "美股", hk: "港股", catalyst: "催化增强", watch: "等待验证", risk: "风险升温", next: "继续看", invalid: "风险点", evidence: "研判依据", timeline: "事件时间线", events: "项已引用事件", fresh: "本期新增，开始跟踪", changed: "判断调整", continuing: "持续跟踪", history: "历史笔记", latest: "最新一期", none: "本期没有该市场的关注股票。", source: "原文", coverage: "资料最早披露于", cutoff: "新闻截止", feedback: "历史反馈", against: "对照", previous: "上期判断", previousWatch: "当时观察", previousRisk: "原风险条件", actual: "本期新事实", assessment: "复核", followups: "上期跟踪", omitted: "本期未入选", supported: "得到支持", partial: "部分支持", contradicted: "出现反证", pending: "尚待验证", insufficient: "资料不足", chartNote: "图表展示已引用事件的披露日期，不代表股价走势或全市场热度。关注理由属于基于现有证据的判断，后续随新事实更新。" },
    en: { title: "Equity notes", empty: "Equity notes will appear after the first edition is generated.", all: "All", cn: "China A", us: "U.S.", hk: "Hong Kong", catalyst: "Catalyst strengthening", watch: "Awaiting confirmation", risk: "Rising risk", next: "Watch next", invalid: "Risk", evidence: "Analysis and evidence", timeline: "Event timeline", events: "cited events", fresh: "New this edition · Tracking begins", changed: "Assessment changed", continuing: "Continuing", history: "Past editions", latest: "Latest edition", none: "No stocks selected in this market this edition.", source: "Source", coverage: "Earliest disclosure", cutoff: "News cutoff", feedback: "Thesis review", against: "Compared with", previous: "Prior thesis", previousWatch: "What we watched", previousRisk: "Original risk", actual: "New evidence", assessment: "Assessment", followups: "Previous watchlist", omitted: "Not selected this edition", supported: "Supported", partial: "Partly supported", contradicted: "Counterevidence", pending: "Awaiting verification", insufficient: "Insufficient evidence", chartNote: "Charts show disclosure dates of cited events, not share prices or market-wide attention. Assessments reflect the available evidence and may change as new facts emerge." },
  };
  function node(tag, className, text = "") {
    const element = document.createElement(tag);
    element.className = className;
    element.textContent = text;
    return element;
  }
  function svgNode(tag, attributes) {
    const element = document.createElementNS("http://www.w3.org/2000/svg", tag);
    for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
    return element;
  }
  function chart(stock, note, t, pick) {
    const figure = node("figure", "equity-chart");
    const start = Date.parse(`${note.chart_start}T00:00:00Z`);
    const end = Date.parse(`${note.briefing_day}T00:00:00Z`);
    const count = Math.round((end - start) / 86400000) + 1;
    const svg = svgNode("svg", { viewBox: "0 0 280 105", role: "img", "aria-label": `${t.timeline} · ${stock.events.length} ${t.events}` });
    svg.append(svgNode("line", { x1: 12, y1: 72, x2: 268, y2: 72, class: "equity-chart-axis" }));
    for (let i = 0; i < count; i += 1) {
      const date = new Date(start + i * 86400000).toISOString().slice(0, 10);
      const events = stock.events.filter((event) => event.date === date);
      const x = 20 + i * 240 / (count - 1);
      const group = svgNode("g", { tabindex: "0", "aria-label": `${date} · ${events.length} ${t.events}` });
      const title = svgNode("title", {});
      title.textContent = `${date} · ${events.length} ${t.events}\n${events.map((event) => pick(event, "fact")).join("\n")}`;
      const height = events.length * 10;
      group.append(title, svgNode("rect", { x: x - 8, y: 72 - Math.max(height, 2), width: 16, height: Math.max(height, 2), rx: 2, class: events.length ? "equity-event-bar" : "equity-event-empty" }));
      if (events.length) {
        const value = svgNode("text", { x, y: 65 - height, "text-anchor": "middle", class: "equity-chart-value" });
        value.textContent = String(events.length);
        group.append(value);
      }
      const label = svgNode("text", { x, y: 94, "text-anchor": "middle", class: "equity-chart-label" });
      label.textContent = date.slice(5).replace("-", "/");
      group.append(label);
      svg.append(group);
    }
    figure.append(svg, node("figcaption", "muted small", t.timeline));
    return figure;
  }
  function render(payload, { lang, onEdition }) {
    const t = labels[lang];
    const editions = lang === "en" ? { morning: "Morning", evening: "Evening" } : { morning: "早间", evening: "晚间" };
    const editionText = (entry) => `${entry.briefing_day} · ${editions[entry.edition]}`;
    const pick = (item, key) => (lang === "en" && item[`${key}_en`]) || item[key];
    const section = node("section", "insight equity-note");
    const note = payload?.note;
    if (!note) {
      section.append(node("p", "empty muted", t.empty));
      return section;
    }
    const dateText = (date) => new Intl.DateTimeFormat(lang === "en" ? "en" : "zh-CN", { timeZone: note.timezone, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(date));
    const header = node("div", "insight-heading");
    header.append(node("h2", "", pick(note, "headline")));
    section.append(header, node("p", "equity-intro", pick(note, "summary")));
    section.append(node("p", "equity-cutoff muted small", `${t.cutoff} ${dateText(note.window_end)} · ${note.timezone}`));
    const toolbar = node("div", "equity-toolbar");
    const filters = node("div", "equity-filters");
    filters.setAttribute("role", "group");
    filters.setAttribute("aria-label", lang === "en" ? "Market" : "市场");
    const stocks = node("div", "equity-stocks");
    const followups = node("section", "equity-followups");
    function showStocks() {
      for (const button of filters.children) button.setAttribute("aria-pressed", String(button.dataset.market === market));
      const selected = note.stocks.filter((stock) => market === "all" || stock.market === market);
      stocks.replaceChildren(...selected.map((stock) => stockCard(stock)));
      if (!selected.length) stocks.append(node("p", "empty muted", t.none));
      followups.replaceChildren();
      const omitted = (note.followups || []).filter((stock) => market === "all" || stock.market === market);
      if (omitted.length) {
        followups.append(node("h2", "equity-followups-title", t.followups));
        for (const stock of omitted) {
          const article = node("article", "equity-followup");
          const identity = node("div", "equity-identity");
          identity.append(node("h3", "item-title", pick(stock, "name")), node("span", "muted small", `${stock.symbol} · ${t[stock.market]} · ${t.omitted}`));
          article.append(identity, renderFeedback(stock.feedback));
          followups.append(article);
        }
      }
    }
    for (const key of ["all", "cn", "hk", "us"]) {
      const button = node("button", "button button-quiet button-small", t[key]);
      button.type = "button";
      button.dataset.market = key;
      button.addEventListener("click", () => { market = key; showStocks(); });
      filters.append(button);
    }
    toolbar.append(filters, node("span", "insight-date", `𝑬 ${t.title} · ${editionText(note)}`));
    section.append(toolbar, stocks, followups);
    function stockCard(stock) {
      const article = node("article", `equity-stock equity-${stock.signal}`);
      const heading = node("div", "equity-stock-heading");
      const identity = node("div", "equity-identity");
      identity.append(node("h3", "item-title", pick(stock, "name")), node("span", "muted small", `${stock.symbol} · ${t[stock.market]}`));
      heading.append(identity, node("span", "equity-signal", t[stock.signal]));
      article.append(heading, node("p", "item-summary equity-reason", pick(stock, "reason")));
      const body = node("div", "equity-stock-body");
      const facts = node("div", "equity-facts");
      facts.append(node("p", "equity-event-count", `${stock.events.length} ${t.events}`));
      for (const [label, key] of [[t.next, "watch"], [t.invalid, "risk"]]) {
        const row = node("p", "equity-condition");
        row.append(node("span", "equity-condition-label", label), document.createTextNode(pick(stock, key)));
        facts.append(row);
      }
      body.append(facts, chart(stock, note, t, pick));
      article.append(body);
      if (stock.feedback) {
        article.append(renderFeedback(stock.feedback));
      } else {
        const change = stock.change === "new" ? t.fresh : t[stock.change];
        article.append(node("p", "equity-change muted small", `${change}${stock.previous_signal && stock.change === "changed" ? ` · ${t[stock.previous_signal]} → ${t[stock.signal]}` : ""}`));
      }
      const evidence = node("section", "equity-evidence");
      evidence.append(node("h4", "", t.evidence));
      evidence.append(...stock.events.map(eventEntry));
      article.append(evidence);
      return article;
    }
    function eventEntry(event) {
      const entry = node("div", "equity-event");
      entry.append(node("time", "muted small", event.date), node("p", "", pick(event, "fact")));
      for (const source of event.sources) {
        try {
          const url = new URL(source.url);
          if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) continue;
          const link = node("a", "equity-source", `${source.source || t.source} ↗`);
          link.href = url.href;
          link.target = "_blank";
          link.rel = "noopener noreferrer";
          entry.append(link);
        } catch { /* A missing source link does not hide the cited fact. */ }
      }
      return entry;
    }
    function renderFeedback(feedback) {
      const block = node("section", `equity-feedback equity-feedback-${feedback.status}`);
      const heading = node("div", "equity-feedback-heading");
      const previousEdition = feedback.previous_edition ? ` · ${editions[feedback.previous_edition]}` : "";
      heading.append(node("h4", "", t.feedback), node("span", "muted small", `${t.against} ${feedback.previous_day}${previousEdition}`), node("span", "equity-feedback-status", t[feedback.status]));
      block.append(heading);
      for (const [label, key] of [[t.previous, "reason"], [t.previousWatch, "watch"], [t.previousRisk, "risk"]]) {
        const row = node("p", "equity-feedback-prior");
        row.append(node("span", "equity-condition-label", label), document.createTextNode(pick(feedback.previous, key)));
        block.append(row);
      }
      if (feedback.events.length) {
        block.append(node("p", "equity-feedback-label", t.actual), ...feedback.events.map(eventEntry));
      }
      const assessment = node("p", "equity-feedback-assessment");
      assessment.append(node("span", "equity-condition-label", t.assessment), document.createTextNode(pick(feedback, "assessment")));
      block.append(assessment);
      return block;
    }
    showStocks();
    section.append(node("p", "equity-footnote muted small", t.chartNote));
    if (note.coverage_start) section.append(node("p", "equity-footnote muted small", `${t.coverage} ${dateText(note.coverage_start)}`));
    if (payload.editions.length > 1) {
      const history = node("label", "equity-history muted small", t.history);
      const select = node("select", "");
      select.setAttribute("aria-label", t.history);
      payload.editions.forEach((entry, index) => {
        const option = node("option", "", `${editionText(entry)}${index === 0 ? ` · ${t.latest}` : ""}`);
        option.value = index === 0 ? "" : entry.window_end;
        option.selected = entry.window_end === note.window_end;
        select.append(option);
      });
      select.addEventListener("change", () => onEdition(select.value));
      history.append(select);
      section.append(history);
    }
    return section;
  }
  return { render };
})();
