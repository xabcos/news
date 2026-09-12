import { $, node, button, withBusy, toast, reportError, openDialog, safeUrl, formatTime, money } from "./ui.js";

// Created only for personal accounts. All requests share the session's abort signal.
export function createAccount(session, onChanged) {
  const dialog = $("#accountDialog");
  let active = true;
  let ordersVersion = 0;
  let paymentVersion = 0;
  let paymentTimer;
  let currentOrder = null;
  let paymentStarted = 0;
  let catalogue = null;
  let offeredPlan = null;
  let creatingOrder = false;

  $("#accountButton").addEventListener("click", () => {
    openDialog(dialog);
    update();
    void withBusy($("#accountButton"), null, async () => {
      await Promise.all([onChanged(), loadPlans(), loadOrders()]);
    });
  }, { signal: session.controller.signal });
  $("#refreshOrdersButton").addEventListener("click", () => withBusy($("#refreshOrdersButton"), null, loadOrders), { signal: session.controller.signal });
  dialog.addEventListener("close", stopPayment, { signal: session.controller.signal });
  document.addEventListener("visibilitychange", () => {
    clearTimeout(paymentTimer);
    if (document.visibilityState === "visible" && active && dialog.open && currentOrder) {
      schedulePayment(paymentVersion);
    }
  }, { signal: session.controller.signal });

  function update() {
    if (!active) return;
    const { user, usage, timezone } = session;
    const summary = node("div", "account-summary");
    summary.append(node("strong", "", user.plan === "pro" ? "Pro" : "Free"));
    if (user.pro_until && user.plan === "pro") {
      const date = new Date(user.pro_until).toLocaleDateString("zh-CN", { timeZone: timezone, year: "numeric", month: "2-digit", day: "2-digit" });
      summary.append(node("p", "muted", `有效期至 ${date}`));
    }
    summary.append(node("p", "", `主题 ${usage.topic_count} / ${user.limits.max_topics} · 今日运行 ${usage.runs_today} / ${user.limits.daily_run_cap}`),
      node("p", "muted", `最短间隔 ${user.limits.min_interval_minutes} 分钟 · 用量于 ${formatTime(usage.reset_at, timezone)} 重置`));
    $("#accountSummary").replaceChildren(summary);
    if (offeredPlan !== user.plan) renderPlans();
  }

  async function loadPlans() {
    catalogue = null;
    $("#planOffer").replaceChildren();
    let data;
    try { data = await session.request("/api/plans"); }
    catch (error) {
      if (active) $("#planOffer").textContent = error.message;
      throw error;
    }
    if (!active) return;
    catalogue = data;
    renderPlans();
  }

  function renderPlans() {
    if (!catalogue) return;
    const { plans, checkout_enabled } = catalogue;
    offeredPlan = session.user.plan;
    const pro = plans.find((plan) => plan.code === "pro");
    const offer = node("div", "plan-offer");
    if (pro) {
      const copy = node("div");
      copy.append(node("strong", "", `Pro · ${money(pro.price_fen)} / ${pro.duration_days} 天`), node("p", "muted", pro.description));
      offer.append(copy);
      if (checkout_enabled) {
        const action = button(offeredPlan === "pro" ? "续费 Pro" : "升级 Pro", createOrder, "button-primary");
        action.disabled = creatingOrder;
        offer.append(action);
      }
      else offer.append(node("span", "muted small", "暂未开放升级"));
    }
    $("#planOffer").replaceChildren(offer);
  }

  async function loadOrders() {
    const version = ++ordersVersion;
    $("#ordersMessage").textContent = "";
    try {
      const orders = await session.request("/api/orders");
      if (!active || version !== ordersVersion) return;
      $("#ordersMessage").textContent = orders.length ? "" : "暂无订单";
      $("#ordersList").replaceChildren(...orders.map((order) => {
        const row = node("li", "order-row");
        const copy = node("div");
        copy.append(node("strong", "", `Pro · ${money(order.amount_fen)}`), node("p", "muted", formatTime(order.created_at, session.timezone)));
        row.append(copy);
        if (order.status === "pending") row.append(button("继续支付", () => openOrder(order.out_trade_no), "button-secondary"));
        else row.append(node("span", "muted small", order.status === "paid" ? "已支付" : "已关闭"));
        return row;
      }));
    } catch (error) {
      if (active && version === ordersVersion) $("#ordersMessage").textContent = error.message;
      throw error;
    }
  }

  async function createOrder() {
    if (creatingOrder) return;
    creatingOrder = true;
    try { await submitOrder(); }
    finally {
      creatingOrder = false;
      if (active) renderPlans();
    }
  }

  async function submitOrder() {
    let result;
    try {
      result = await session.request("/api/orders", { method: "POST", body: { plan_code: "pro" } });
    } catch (error) {
      // The provider may fail after the local order was saved; keep it visible for retry.
      if (active) await loadOrders().catch(() => {});
      throw error;
    }
    if (!active) return;
    showOrder(result);
    if (result.order.status === "pending") await loadOrders();
    else await refreshAfterPayment(result, paymentVersion);
  }

  async function openOrder(number) {
    const version = ++paymentVersion;
    const result = await session.request(`/api/orders/${encodeURIComponent(number)}`);
    if (!active || version !== paymentVersion) return;
    showOrder(result);
    if (result.order.status !== "pending") {
      await refreshAfterPayment(result, paymentVersion);
    }
  }

  function stopPayment() {
    clearTimeout(paymentTimer);
    paymentVersion += 1;
    currentOrder = null;
    $("#paymentPanel").classList.add("hidden");
    $("#paymentContent").replaceChildren();
  }

  function showOrder(result) {
    stopPayment();
    currentOrder = result.order.status === "pending" ? result.order.out_trade_no : null;
    paymentStarted = Date.now();
    $("#paymentPanel").classList.remove("hidden");
    renderPayment(result);
    if (result.order.status === "pending") schedulePayment(paymentVersion);
  }

  function renderPayment({ order, payment }) {
    const content = node("div", "payment-content");
    content.append(node("strong", "", `Pro · ${money(order.amount_fen)}`));
    if (order.status !== "pending") {
      content.append(node("p", "", order.status === "paid" ? "支付成功，Pro 已开通" : "订单已关闭，可重新发起支付"));
    } else {
      const hasQr = Boolean(payment?.code_url);
      if (hasQr) {
        const qr = node("img");
        qr.alt = "微信支付二维码";
        qr.src = `/api/orders/${encodeURIComponent(order.out_trade_no)}/qr.svg`;
        qr.addEventListener("error", () => qr.replaceWith(node("p", "muted", "二维码暂时无法载入，请重新打开订单")));
        content.append(qr);
      }
      content.append(node("p", "muted", hasQr ? `微信扫码支付 · ${formatTime(order.expires_at, session.timezone)} 前有效` : "支付信息暂不可用，可重试升级以重新获取"));
      const url = safeUrl(payment?.h5_url);
      if (url) {
        const link = node("a", "", "打开支付页面");
        link.href = url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        content.append(link);
      }
      content.append(node("p", "muted", "等待支付…"));
      content.lastChild.id = "paymentStatus";
      content.append(button("检查支付结果", () => checkPayment(paymentVersion, true)));
    }
    $("#paymentContent").replaceChildren(content);
  }

  function schedulePayment(version) {
    clearTimeout(paymentTimer);
    if (!active || !dialog.open || document.visibilityState !== "visible" || version !== paymentVersion) return;
    if (Date.now() - paymentStarted >= 10 * 60 * 1000) {
      if ($("#paymentStatus")) $("#paymentStatus").textContent = "自动查询已暂停，可手动检查支付结果";
      return;
    }
    paymentTimer = setTimeout(() => {
      if (document.visibilityState === "visible") void checkPayment(version).catch(reportError);
    }, 5000);
  }

  async function refreshAfterPayment(result, version) {
    try {
      await onChanged();
      await Promise.all([loadOrders(), loadPlans()]);
      if (!active || version !== paymentVersion) return;
      renderPayment(result);
      if (result.order.status === "paid") toast("支付成功，Pro 已开通");
    } catch (error) {
      if (!active || version !== paymentVersion) return;
      renderPayment(result);
      $("#paymentContent .payment-content").append(button("刷新套餐与用量", () => refreshAfterPayment(result, version), "button-secondary"));
      reportError(error);
    }
  }

  async function checkPayment(version, manual = false) {
    if (!active || !currentOrder || version !== paymentVersion) return;
    clearTimeout(paymentTimer);
    try {
      const result = await session.request(`/api/orders/${encodeURIComponent(currentOrder)}`);
      if (!active || version !== paymentVersion) return;
      if (result.order.status === "pending") {
        if ($("#paymentStatus")) $("#paymentStatus").textContent = "尚未收到付款，完成支付后会自动更新";
        if (manual) paymentStarted = Date.now();
        schedulePayment(version);
      } else {
        paymentVersion += 1;
        currentOrder = null;
        renderPayment(result);
        await refreshAfterPayment(result, paymentVersion);
      }
    } catch (error) {
      if (!active || version !== paymentVersion) return;
      if ($("#paymentStatus")) $("#paymentStatus").textContent = error.message;
      if (error.status !== 404) schedulePayment(version);
    }
  }

  return {
    update,
    destroy() { active = false; ordersVersion += 1; stopPayment(); },
  };
}
