(() => {
  "use strict";
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const root = document.documentElement;

  /* Theme: the stored choice, else the system appearance. */
  const themeBtn = $("#theme-btn");
  if (themeBtn) {
    const sync = () => {
      const dark = root.dataset.theme === "dark";
      $("use", themeBtn).setAttribute("href", dark ? "#i-sun" : "#i-moon");
      themeBtn.setAttribute("aria-label", dark ? "Use the light appearance" : "Use the dark appearance");
    };
    sync();
    themeBtn.addEventListener("click", () => {
      const next = root.dataset.theme === "dark" ? "light" : "dark";
      try { localStorage.setItem("alttabio-theme", next); } catch (e) {}
      root.classList.add("theme-switching");
      root.dataset.theme = next;
      sync();
      requestAnimationFrame(() => requestAnimationFrame(() => root.classList.remove("theme-switching")));
    });
  }

  /* Header */
  const header = $(".site-header");
  const menuBtn = $("#menu-btn");
  if (header) {
    const onScroll = () => header.classList.toggle("stuck", window.scrollY > 4);
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
  }
  if (header && menuBtn) {
    const setMenu = (open) => {
      header.classList.toggle("is-open", open);
      menuBtn.setAttribute("aria-expanded", String(open));
      menuBtn.setAttribute("aria-label", open ? "Close menu" : "Open menu");
      $("use", menuBtn).setAttribute("href", open ? "#i-x" : "#i-list");
    };
    menuBtn.addEventListener("click", () => setMenu(!header.classList.contains("is-open")));
    $$(".navlinks a", header).forEach((a) => a.addEventListener("click", () => setMenu(false)));
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape" || !header.classList.contains("is-open")) return;
      const restore = header.contains(document.activeElement);
      setMenu(false);
      if (restore) menuBtn.focus();
    });
    window.matchMedia("(min-width: 860px)").addEventListener("change", (e) => { if (e.matches) setMenu(false); });
  }

  /* Copy buttons */
  $$("[data-copy]").forEach((button) => {
    const source = document.getElementById(button.dataset.copy);
    const status = $(".copy-status", button.closest(".cmd"));
    let timer = 0;
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(source.textContent.trim());
      } catch (e) {
        const range = document.createRange();
        range.selectNodeContents(source);
        getSelection().removeAllRanges();
        getSelection().addRange(range);
        return;
      }
      button.setAttribute("data-copied", "");
      button.setAttribute("aria-label", "Copied");
      if (status) status.textContent = "Copied to the clipboard";
      clearTimeout(timer);
      timer = setTimeout(() => {
        button.removeAttribute("data-copied");
        button.setAttribute("aria-label", "Copy the command");
        if (status) status.textContent = "";
      }, 1800);
    });
  });

  /* The install sheet opens in place of the #install jump when dialogs are available. */
  $$("[data-sheet]").forEach((link) => {
    const sheet = document.getElementById(link.dataset.sheet);
    if (!sheet || typeof sheet.showModal !== "function") return;
    link.addEventListener("click", (e) => {
      e.preventDefault();
      sheet.showModal();
    });
    sheet.addEventListener("click", (e) => { if (e.target === sheet) sheet.close(); });
  });
})();
