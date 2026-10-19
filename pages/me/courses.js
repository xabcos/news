import { $, node, withBusy, toast } from "./ui.js";

const sources = { seed: "初始课程", request: "你开的", news: "按新闻自动开的" };

// 学习's courses for an administrator: a sentence opens one, which XABC designs in a minute or two;
// another sentence turns one, its brief shown to read first; and any can be hidden, which stops it
// growing and takes it off the pages. Requests share the session's abort signal.
export function createCourses(session) {
  const signal = session.controller.signal;
  let polling = 0;
  // What the administrator has open survives every redraw, polling's included: the briefs being
  // read, and the sentences being typed to turn a course.
  const reading = new Set();
  const drafts = new Map();

  $("#courseForm").addEventListener("submit", (event) => {
    event.preventDefault();
    void withBusy($("#courseForm button"), "提交中…", async () => {
      await session.request("/api/admin/courses", { method: "POST", body: { idea: $("#courseIdea").value } });
      $("#courseIdea").value = "";
      toast("XABC 正在设计这门课，一两分钟后刷新可见");
      await load();
    });
  }, { signal });

  // While XABC designs a course, look again every 15 seconds until it shows or fails; a look that
  // fails tries again, until the session ends.
  function poll() {
    clearTimeout(polling);
    polling = setTimeout(() => void load().catch(() => { if (!signal.aborted) poll(); }), 15000);
  }

  async function load() {
    const { courses, designing, error } = await session.request("/api/admin/courses");
    if (signal.aborted) return;
    clearTimeout(polling);
    if (designing) poll();
    $("#coursesMeta").textContent = designing ? `正在设计：${designing}`
      : error ? `上一门没开成：${error}` : `${courses.length} 门课程`;
    const active = document.activeElement;
    const typing = active.name === "change" && $("#courseList").contains(active) ? active.closest("li").dataset.course : null;
    const selection = typing && [active.selectionStart, active.selectionEnd, active.selectionDirection];
    let focused = null;
    $("#courseList").replaceChildren(...courses.map((course) => {
      const row = node("li", "item-row");
      row.dataset.course = course.id;
      const title = node("a", "", course.title);
      title.href = `/learn/${encodeURIComponent(course.id)}`;
      title.target = "_blank";
      title.rel = "noopener";
      const path = `/api/admin/courses/${encodeURIComponent(course.id)}`;
      const turn = node("button", "button button-quiet button-small", "调整");
      turn.type = "button";
      const toggle = node("button", "button button-quiet button-small", course.visible ? "隐藏" : "显示");
      toggle.type = "button";
      toggle.addEventListener("click", () => void withBusy(toggle, "…", async () => {
        await session.request(path, { method: "PATCH", body: { visible: !course.visible } });
        await load();
      }));
      const actions = node("div", "item-actions");
      actions.append(turn, toggle);
      const about = node("div");
      about.append(title, node("p", "item-meta muted", `${course.group} · ${sources[course.source]} · ${course.lessons} 课${course.visible ? "" : " · 已隐藏"}`));
      const line = node("div", "course-line");
      line.append(about, actions);
      // Its brief, to read before turning it.
      const brief = node("details", "course-brief");
      brief.open = reading.has(course.id);
      brief.addEventListener("toggle", () => brief.open ? reading.add(course.id) : reading.delete(course.id));
      brief.append(node("summary", "muted small", "课程说明"), node("p", "small", course.brief));
      const form = node("form", drafts.has(course.id) ? "course-turn" : "course-turn hidden");
      const change = Object.assign(node("input"), { name: "change", minLength: 4, maxLength: 200, required: true, placeholder: "一句话说怎么调整，比如：服务端改用 Rust", value: drafts.get(course.id) ?? "" });
      change.setAttribute("aria-label", `调整「${course.title}」`);
      change.addEventListener("input", () => drafts.set(course.id, change.value));
      if (course.id === typing && drafts.has(course.id)) focused = change;
      const send = node("button", "button button-primary button-small", "调整");
      send.type = "submit";
      form.append(change, send);
      turn.addEventListener("click", () => {
        form.classList.toggle("hidden");
        if (form.classList.contains("hidden")) drafts.delete(course.id);
        else {
          drafts.set(course.id, change.value);
          change.focus();
        }
      });
      form.addEventListener("submit", (event) => {
        event.preventDefault();
        void withBusy(send, "提交中…", async () => {
          await session.request(`${path}/direction`, { method: "POST", body: { change: change.value } });
          drafts.delete(course.id);
          toast("XABC 正在改写这门课，一两分钟后刷新可见");
          await load();
        });
      });
      row.append(line, brief, form);
      return row;
    }));
    if (focused) {
      focused.focus();
      focused.setSelectionRange(...selection);
    }
  }

  return { load };
}
