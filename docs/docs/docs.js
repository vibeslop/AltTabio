(() => {
  "use strict";
  const root = document.documentElement;
  const picks = [...document.querySelectorAll("[data-platform-pick]")];

  /* The system switch. The head script chose the first one; nothing is remembered. */
  const show = (platform) => {
    root.dataset.platform = platform;
    picks.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.platformPick === platform)));
  };
  show(root.dataset.platform === "mac" ? "mac" : "windows");
  picks.forEach((b) => b.addEventListener("click", () => show(b.dataset.platformPick)));

  /* A link into the other system's instructions switches to that system first, so its target is
     there to scroll to. This runs before site.js follows the link. */
  const reveal = (id) => {
    if (id === "windows" || id === "mac") { show(id); return true; }
    const only = document.getElementById(id)?.closest("[data-only]");
    if (!only || only.dataset.only === root.dataset.platform) return false;
    show(only.dataset.only);
    return true;
  };
  document.addEventListener("click", (e) => {
    const link = e.target.closest('a[href^="#"]');
    if (link && link.hash.length > 1) reveal(decodeURIComponent(link.hash.slice(1)));
  }, true);
  window.addEventListener("hashchange", () => {
    const id = decodeURIComponent(location.hash.slice(1));
    if (id && reveal(id)) document.getElementById(id)?.scrollIntoView();
  });

  /* The contents mark the topic at the top of the window. */
  const links = [...document.querySelectorAll(".toc a")];
  let frame = 0;
  const mark = () => {
    frame = 0;
    const shown = links.filter((l) => l.getClientRects().length);
    // At the very end, the last topic is current even when it is too short to reach the top.
    const atEnd = window.innerHeight + window.scrollY >= root.scrollHeight - 2;
    let current = atEnd ? shown[shown.length - 1] : shown[0];
    if (!atEnd) {
      for (const link of shown) {
        const section = document.getElementById(link.hash.slice(1));
        if (section && section.getBoundingClientRect().top <= 120) current = link;
      }
    }
    links.forEach((l) => (l === current ? l.setAttribute("aria-current", "location") : l.removeAttribute("aria-current")));
  };
  const queue = () => { if (!frame) frame = requestAnimationFrame(mark); };
  window.addEventListener("scroll", queue, { passive: true });
  window.addEventListener("resize", queue);
  picks.forEach((b) => b.addEventListener("click", queue));
  mark();
})();
