// Vigie landing page: reveal on first view, and the status strip that ticks like the app's.
// Nothing here is needed to read the page: without script, everything is shown.
(function () {
  "use strict";
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var parts = document.querySelectorAll(".rise, .hero-window");

  if (reduce || !("IntersectionObserver" in window)) {
    parts.forEach(function (el) { el.classList.add("seen"); });
  } else {
    var seen = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        // Siblings that arrive together rise one after the other.
        var el = entry.target;
        var index = Array.prototype.indexOf.call(el.parentNode.children, el);
        el.style.transitionDelay = Math.min(index, 5) * 70 + "ms";
        el.classList.add("seen");
        seen.unobserve(el);
      });
    }, { rootMargin: "0px 0px -8% 0px" });
    parts.forEach(function (el) { seen.observe(el); });
  }

  // The status strip: Vigie's own cost, as its status bar shows it, within the measured range.
  var cpu = document.querySelector("[data-cpu]");
  var mem = document.querySelector("[data-mem]");
  if (!cpu || !mem || reduce) return;
  var comma = document.documentElement.lang === "fr";
  var format = function (value, digits) {
    var text = value.toFixed(digits);
    return comma ? text.replace(".", ",") : text;
  };
  var load = 0.3;
  var memory = 9.2;
  setInterval(function () {
    if (document.hidden) return;
    load = Math.min(0.6, Math.max(0.1, load + (Math.random() - 0.5) * 0.2));
    memory = Math.min(9.6, Math.max(9.0, memory + (Math.random() - 0.5) * 0.1));
    cpu.textContent = format(load, 1);
    mem.textContent = format(memory, 1);
  }, 1000);
})();
