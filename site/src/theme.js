// Runs before the page paints so the saved terminal theme shows without a flash.
// The only thing this site stores: the theme you picked, in this browser.
(function () {
  var t = null;
  try { t = localStorage.getItem("dredge-theme"); } catch (e) {}
  if (!t || t === "auto") t = matchMedia("(prefers-color-scheme: light)").matches ? "paper" : "graphite";
  document.documentElement.setAttribute("data-theme", t);
})();
