"use strict";

// Pick the first-paint room before the deferred state request completes.
// The API remains authoritative once it supplies the cat's presence.
try {
  const hour = new Date().getHours();
  if (hour < 6 || hour >= 23) document.documentElement.dataset.catInitialPresence = "home_sleeping";
} catch (_) {
  // Keep the daytime fallback if the clock is unavailable.
}
