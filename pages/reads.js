"use strict";

PublicPage.init();

const english = document.documentElement.lang === "en";
try { localStorage.setItem("news.lang", english ? "en" : "zh"); } catch { /* Reading works without storage. */ }

const activeTab = document.querySelector('.tabs [aria-current="true"]');
activeTab?.scrollIntoView({ block: "nearest", inline: "center" });
