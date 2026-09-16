"use strict";

// Pick the first-paint room before the deferred state request completes.
// The API remains authoritative once it supplies the travel's presence.
try {
  const hour = new Date().getHours();
  if (hour < 6 || hour >= 23) document.documentElement.dataset.travelInitialPresence = "home_sleeping";
} catch (_) {
  // Keep the daytime fallback if the clock is unavailable.
}

// A photo remembered from the last visit keeps the default art hidden until travel.js
// has shown it again or the API has cleared it.
try {
  if (window.localStorage.getItem("travel:latestPhoto")) document.documentElement.dataset.travelPhotoPending = "1";
} catch (_) {
  // Storage may be blocked; the page then paints the room as before.
}
