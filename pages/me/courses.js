import { $, node, withBusy, toast } from "./ui.js";

const sources = { seed: "初始课程", request: "你开的", news: "按新闻自动开的" };

// 学习's courses for an administrator: a sentence opens one, which XABC designs in a minute or two,
// and any can be hidden, which stops it growing and takes it off the pages. Requests share the
// session's abort signal.
export function createCourses(session) {
  const signal = session.controller.signal;
  let polling = 0;

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
    $("#courseList").replaceChildren(...courses.map((course) => {
      const row = node("li", "item-row");
      const title = node("a", "", course.title);
      title.href = `/learn/${encodeURIComponent(course.id)}`;
      title.target = "_blank";
      title.rel = "noopener";
      const toggle = node("button", "button button-quiet button-small", course.visible ? "隐藏" : "显示");
      toggle.type = "button";
      toggle.addEventListener("click", () => void withBusy(toggle, "…", async () => {
        await session.request(`/api/admin/courses/${encodeURIComponent(course.id)}`, { method: "PATCH", body: { visible: !course.visible } });
        await load();
      }));
      const actions = node("div", "item-actions");
      actions.append(toggle);
      row.append(title, node("p", "item-meta muted", `${course.group} · ${sources[course.source]} · ${course.lessons} 课${course.visible ? "" : " · 已隐藏"}`), actions);
      return row;
    }));
  }

  return { load };
}
