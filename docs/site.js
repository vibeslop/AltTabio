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
      // The browser's own bars match the header: the stage's colour, or the page's on the Docs.
      if ($(".site-head")) $$('meta[name="theme-color"]').forEach((m) => { m.content = dark ? "#07090d" : "#f8fafc"; });
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

  /* Copy buttons */
  $$("[data-copy]").forEach((button) => {
    const source = document.getElementById(button.dataset.copy);
    const status = $(".copy-status", button.closest(".cmd"));
    const label = button.getAttribute("aria-label");
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
        button.setAttribute("aria-label", label);
        if (status) status.textContent = "";
      }, 1800);
    });
  });

  /* ─────────────────────────────────────────────────────────
   * INSTALL COMMAND BORDER BEAM
   *
   * Two lines of light run clockwise round the install command's
   * border, half a lap apart, each fading from an azure tail to an
   * off-white head. Half a lap apart means that when one head is in
   * the top-right corner the other is in the bottom-left, and there
   * both speed up a little. The copy button glows faintly as a line
   * passes it, as bright as the line is at that point.
   *
   *    0%   heads at the top-left and bottom-right, setting off
   *         right along the top and left along the bottom
   *   ~90%  heads round the top-right and bottom-left corners (surge);
   *         where depends on the box's shape, ~90% for the hero's
   *  100%   half a lap: the lines have swapped places; loop
   *
   * With reduced motion there is no beam, only the plain border.
   * ───────────────────────────────────────────────────────── */
  const BEAM = {
    lap:       11000,              // ms for one line to go all the way round, before the surges
    length:    9,                  // % of the border each line covers
    bands:     12,                 // steps from tail to head, so the fade reads as a gradient
    surge:     0.5,                // extra speed through the two corners: 0.5 is half again as fast
    spread:    0.02,               // how much of a lap the surge spans either side of the corner
    tail:      [0.38, 0.1, 248],   // deep azure, as OKLCH
    head:      [0.6, 0.13, 245],   // azure, never white, so it stays in the background
    peak:      0.6,                // the head's opacity; the tail fades from a fifth of it
    glow:      0.55,               // share of each line, from its head, that glows
    glowAlpha: 0.2,                // how strong the glow is
    button:    1,                  // the copy button's highlight at most, as the head passes it
    radius:    11.5,               // the ring's corner radius: the box's 12px less half the line
  };
  const SVG = "http://www.w3.org/2000/svg";
  const tint = (t) => `oklch(${BEAM.tail.map((v, i) => +(v + (BEAM.head[i] - v) * t).toFixed(4)).join(" ")})`;
  if (!matchMedia("(prefers-reduced-motion: reduce)").matches && Element.prototype.animate) {
    $$(".hero-cmd, .closing-cmd").forEach((box) => {
      // Each line is a stack of dashes that all end at the head, shorter and paler towards it.
      // A dash every 50 of the ring's 100 units makes the two lines.
      const lines = [];
      const layer = (glow) => {
        const svg = document.createElementNS(SVG, "svg");
        svg.setAttribute("class", glow ? "beam glow" : "beam");
        svg.setAttribute("aria-hidden", "true");
        box.prepend(svg);
        return svg;
      };
      const dash = (svg, length, color, alpha) => {
        const ring = document.createElementNS(SVG, "rect");
        ring.setAttribute("pathLength", "100");
        ring.style.stroke = color;
        ring.style.strokeOpacity = alpha;
        ring.style.strokeDasharray = `${length} ${50 - length}`;
        svg.append(ring);
        lines.push([ring, length]);
      };
      const lit = layer(false);
      dash(layer(true), BEAM.length * BEAM.glow, tint(0.3), BEAM.glowAlpha);
      for (let i = 0; i < BEAM.bands; i++) {
        const t = i / (BEAM.bands - 1);
        dash(lit, BEAM.length * (1 - i / BEAM.bands), tint(t), BEAM.peak * (0.2 + 0.8 * t));
      }

      // The ring starts just after the top-left corner and runs clockwise. Where the top-right
      // corner falls on it depends on the box's shape, so the timing comes from its size.
      const button = $(".icon-btn", box);
      let running = [];
      const build = () => {
        running.forEach((a) => a.cancel());
        running = [];
        const r = BEAM.radius;
        const w = box.offsetWidth - 1, h = box.offsetHeight - 1;
        if (w <= 2 * r || h < 2 * r) return;
        const ring = 2 * (w + h) - (8 - 2 * Math.PI) * r;
        const corner = (w - 2 * r + (Math.PI * r) / 4) / ring;
        const near = (p, c) => {
          const d = ((((p - c) % 1) + 1.5) % 1) - 0.5;
          return Math.exp(-0.5 * (d / BEAM.spread) ** 2);
        };
        const speed = (p) => 1 + BEAM.surge * (near(p, corner) + near(p, corner + 0.5));
        // Half a lap in small steps, each taking longer the slower the line goes there.
        const steps = 120, at = [0];
        for (let k = 1; k <= steps; k++) {
          const a = (0.5 * (k - 1)) / steps, b = (0.5 * k) / steps;
          at.push(at[k - 1] + ((b - a) * (1 / speed(a) + 1 / speed(b))) / 2);
        }
        const total = at[steps];
        const duration = BEAM.lap * total;
        lines.forEach(([ring, length]) => {
          const frames = at.map((t, k) => ({ offset: t / total, strokeDashoffset: `${length - (50 * k) / steps}` }));
          running.push(ring.animate(frames, { duration, iterations: Infinity }));
        });
        // The copy button sits on the right side: it lights up as a head reaches it and dims
        // with the tail behind it.
        if (button) {
          const y = Math.min(h - r, Math.max(r, button.offsetTop + button.offsetHeight / 2 - 0.5));
          const spot = (w - 2 * r + (Math.PI * r) / 2 + (y - r)) / ring;
          const tailLength = BEAM.length / 100;
          const lit = (head) => {
            const d = ((((head - spot) % 1) + 1.5) % 1) - 0.5;
            return d < 0 ? Math.exp(-0.5 * (d / 0.01) ** 2) : Math.max(0, 1 - d / tailLength) ** 2;
          };
          const frames = at.map((t, k) => {
            const g = BEAM.button * Math.max(lit((0.5 * k) / steps), lit((0.5 * k) / steps + 0.5));
            const f = (v) => v.toFixed(3);
            return {
              offset: t / total,
              color: `oklch(${f(0.85 + 0.07 * g)} ${f(0.09 * g)} 240)`,
              filter: `drop-shadow(0 0 ${f(6 * g)}px oklch(0.65 0.14 245 / ${f(g)}))`,
              boxShadow: `inset 0 0 0 1px oklch(0.6 0.13 245 / ${f(0.6 * g)}), 0 0 ${f(14 * g)}px oklch(0.6 0.13 245 / ${f(0.35 * g)})`,
            };
          });
          running.push(button.animate(frames, { duration, iterations: Infinity }));
        }
      };
      build();
      new ResizeObserver(build).observe(box);
      // Off screen it stops, so it costs nothing there.
      new IntersectionObserver(([e]) => running.forEach((a) => (e.isIntersecting ? a.play() : a.pause()))).observe(box);
    });
  }

  /* A link to a section of this page jumps at once; where the browser can, the jump cross-fades.
     The browser still follows the link itself, so history, :target and focus behave as usual. */
  if (document.startViewTransition) {
    document.addEventListener("click", (e) => {
      const link = e.target.closest('a[href^="#"]');
      if (!link || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      const target = link.hash.length > 1 && document.getElementById(decodeURIComponent(link.hash.slice(1)));
      if (!target) return;
      e.preventDefault();
      document.startViewTransition(() => {
        if (location.hash === link.hash) target.scrollIntoView();
        else location.hash = link.hash;
      });
    });
  }

  /* The switcher demo. Apps and windows are kept in most-recent order, as the real switcher
     keeps them; switching moves the chosen window to the front of both. */
  const desk = $(".desk");
  if (desk) {
    const panel = $(".sw", desk);
    const front = $(".desk-front", desk);
    const reopen = $(".reopen", desk);
    const status = $("#demo-status");
    const keys = $("#demo-keys");
    // Icon gradients live in one always-rendered <defs>: a gradient inside a hidden copy
    // of an icon would leave every other copy unfilled.
    const defs = [];
    // A gradient in the shared <defs>, from oklch stops; returns its url().
    const grad = (stops, { diagonal = false, radial = false, dir = [0, 0, diagonal ? 1 : 0, 1] } = {}) => {
      const id = `demo-g${defs.length}`;
      const s = stops.map((c, i) => `<stop offset="${i / (stops.length - 1)}" stop-color="oklch(${c})"/>`).join("");
      defs.push(radial ? `<radialGradient id="${id}" cx="0.35" cy="0.3" r="0.8">${s}</radialGradient>`
        : `<linearGradient id="${id}" x1="${dir[0]}" y1="${dir[1]}" x2="${dir[2]}" y2="${dir[3]}">${s}</linearGradient>`);
      return `url(#${id})`;
    };
    const stroke = (color = "white", width = 2.6) => `fill="none" stroke="${color}" stroke-width="${width}" stroke-linecap="round" stroke-linejoin="round"`;
    const svg = (body) => `<svg viewBox="0 0 48 48" aria-hidden="true">${body}</svg>`;

    // macOS 26, clear: a frosted squircle with a lit rim and a white symbol, so the icons sit
    // back and the window titles carry the panel.
    const squircle = `M${Array.from({ length: 96 }, (_, i) => {
      const t = (i / 96) * 2 * Math.PI;
      const f = (v) => (24 + 19.5 * Math.sign(v) * Math.abs(v) ** 0.4).toFixed(2);
      return `${f(Math.cos(t))} ${f(Math.sin(t))}`;
    }).join("L")}Z`;
    const frost = grad(["1 0 0 / 0.32", "1 0 0 / 0.1"]);
    const rim = grad(["1 0 0 / 0.75", "1 0 0 / 0.08", "1 0 0 / 0.4"], { diagonal: true });
    const clear = (glyph) => svg(`<path d="${squircle}" fill="${frost}"/><path d="${squircle}" fill="none" stroke="${rim}" stroke-width="1"/><g opacity="0.95">${glyph}</g>`);
    // A clip shape in the shared <defs>, for the same reason the gradients live there; returns its url().
    const clip = (shape) => {
      const id = `demo-c${defs.length}`;
      defs.push(`<clipPath id="${id}">${shape}</clipPath>`);
      return `url(#${id})`;
    };
    // Two beamed notes, shared by both systems' Music: tilted heads, stems as heavy as the other
    // icons' lines, and a beam nearly twice a stem. Each head reaches exactly as far right as its
    // stem and the stem stops at the head's widest point, so no corner of a stem shows.
    const notes = `<g transform="translate(-0.2 0.4)"><path d="M20.2 15.3l13.2-3v3.2l-13.2 3z" fill="white" stroke="white" stroke-width="1.2" stroke-linejoin="round"/><path fill="none" stroke="white" stroke-width="2.4" d="M20.8 17v14.5M32.8 14v14.5"/><ellipse cx="18.18" cy="32" rx="3.9" ry="3" transform="rotate(-18 18.18 32)" fill="white"/><ellipse cx="30.18" cy="29" rx="3.9" ry="3" transform="rotate(-18 30.18 29)" fill="white"/></g>`;
    const glass = {
      // The ridge meets the frame on its straight sides, clear of the corners, so each end is a clean join.
      photos: clear(`<rect x="11.5" y="14.5" width="25" height="19" rx="4.5" ${stroke("white", 2.4)}/><circle cx="17.5" cy="19.6" r="2.1" fill="white"/><path ${stroke("white", 2.4)} d="M11.5 28.5l5.5-4.5 4.5 4 6-6.5 9 7"/>`),
      web: clear(`<circle cx="24" cy="24" r="11.5" ${stroke("white", 2.4)}/><path ${stroke("white", 2.4)} d="M12.5 24h23M24 12.5c-4.5 4-4.5 19 0 23M24 12.5c4.5 4 4.5 19 0 23"/>`),
      mail: clear(`<rect x="11.5" y="15" width="25" height="18" rx="3.5" fill="white"/><path ${stroke("oklch(0.45 0 0 / 0.55)", 2)} d="M13 17l11 8.5 11-8.5"/>`),
      // A sheet in one line weight: a rule under its heading and two lines of writing.
      notes: clear(`<rect x="14.5" y="12.5" width="19" height="23" rx="4" ${stroke("white", 2.4)}/><path ${stroke("white", 2.4)} d="M14.5 18.5h19M19 24.5h10M19 30h6"/>`),
      music: clear(notes),
    };

    // The browser's ribbon: from a point at the upper left it runs clockwise 300 degrees round the
    // centre, its inner edge drawing in from 18 to 11, and ends round at the lower left.
    const swirl = (() => {
      const at = (deg, r) => { const a = (deg * Math.PI) / 180; return `${(24 + r * Math.cos(a)).toFixed(2)} ${(24 + r * Math.sin(a)).toFixed(2)}`; };
      const steps = 60;
      const outer = Array.from({ length: steps + 1 }, (_, i) => at(200 + (300 * i) / steps, 18));
      const inner = Array.from({ length: steps + 1 }, (_, i) => at(200 + (300 * i) / steps, 18 - 7 * (i / steps) ** 0.8)).reverse();
      return `M${outer.join("L")}A3.5 3.5 0 0 1 ${inner[0]}L${inner.join("L")}Z`;
    })();
    // Windows 11, Fluent: no tile, the object itself, coloured and lit from above.
    const hills = grad(["0.86 0.14 150", "0.64 0.15 160"]);
    const fluent = {
      // The hills run past the frame and are cut by it, so the ground fills the picture to its corners.
      photos: svg(`<rect x="6" y="9" width="36" height="30" rx="5" fill="${grad(["0.84 0.1 225", "0.64 0.15 245"])}"/><g clip-path="${clip('<rect x="6" y="9" width="36" height="30" rx="5"/>')}"><circle cx="15.5" cy="17.5" r="4" fill="oklch(0.94 0.13 95)"/><path d="M2 44V37l13-9 7 6 7.5-11.5L46 37v7z" fill="${hills}" stroke="${hills}" stroke-width="2" stroke-linejoin="round"/></g>`),
      // A browser's kind of mark: a solid disc, a bright ribbon sweeping round inside it, and a
      // light core. The Earth, a gridded globe and a tabbed window read as Maps, Internet
      // Explorer and a folder; an open ring reads as a spinner.
      web: svg(`<circle cx="24" cy="24" r="18" fill="${grad(["0.5 0.19 270", "0.34 0.17 275"], { diagonal: true })}"/><path d="${swirl}" fill="${grad(["0.9 0.12 190", "0.72 0.16 215", "0.56 0.19 245"])}"/><circle cx="24" cy="24" r="9.5" fill="${grad(["0.99 0.01 220", "0.88 0.06 225", "0.7 0.12 240"], { radial: true })}"/>`),
      mail: svg(`<rect x="6" y="11" width="36" height="27" rx="4" fill="${grad(["0.68 0.15 245", "0.5 0.18 258"])}"/><path d="M6 15a4 4 0 0 1 4-4h28a4 4 0 0 1 4 4v1.5L24 29 6 16.5z" fill="${grad(["0.88 0.07 230", "0.74 0.12 240"])}"/>`),
      notes: svg(`<rect x="10" y="7" width="28" height="35" rx="4" fill="${grad(["0.95 0.1 95", "0.86 0.15 85"])}"/><path d="M10 11a4 4 0 0 1 4-4h20a4 4 0 0 1 4 4v4H10z" fill="oklch(0.74 0.15 70)"/><path ${stroke("oklch(0.6 0.08 80 / 0.7)", 2)} d="M15.5 23h17M15.5 29h17M15.5 35h10"/>`),
      music: svg(`<circle cx="24" cy="24" r="18" fill="${grad(["0.72 0.17 300", "0.5 0.21 285"], { diagonal: true })}"/><g transform="translate(24 24) scale(0.8) translate(-24 -24)">${notes}</g>`),
    };

    // What each window shows: an ordinary week with a trip coming up. Nothing on this desktop
    // needs explaining to someone who has never written code.
    // The address bar is left out: an invented address reads as a placeholder.
    const page = (inner) => `<div class="main">${inner}</div>`;
    const table = (rows) => `<table>${rows.map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`).join("")}</table>`;
    const mail = (pick, subject, from, text) => `<div class="side inbox">${[["Maya Chen", "Re: Friday dinner"], ["Casa Alfama", "Your stay is confirmed"], ["Lisbon Air", "Your booking LX4821"], ["Sam Ortiz", "Lunch Thursday?"], ["Rui Costa", "Photos from the trip"]].map(([n, s], i) => `<p${i === pick ? ' class="on"' : ""}><b>${n}</b>${s}</p>`).join("")}</div><div class="main"><h4>${subject}</h4><p class="meta">${from}</p>${text.map((t) => `<p>${t}</p>`).join("")}</div>`;
    // A photo album: the library beside a grid of pictures, each a wash of one hue.
    const album = (open, meta, hues) => `<div class="side tree">${["Library", "Favourites", "Beach weekend", "Garden in May", "Birthday dinner", "Scans"].map((a) => `<p${a === open ? ' class="on"' : ""}>${a}</p>`).join("")}</div><div class="main"><h4>${open}</h4><p class="meta">${meta}</p><div class="shots">${hues.map((h, i) => `<i style="background:linear-gradient(160deg, oklch(${0.87 - (i % 3) * 0.03} 0.09 ${h}), oklch(${0.64 - (i % 2) * 0.05} 0.14 ${h + 14}))"></i>`).join("")}</div></div>`;
    const songs = (name, meta, rows) => `<div class="main"><h3>${name}</h3><p class="meta">${meta}</p>${table(rows)}</div>`;
    const bodies = {
      "Beach weekend": album("Beach weekend", "June · 48 photos", [230, 85, 215, 60, 245, 200, 75, 20]),
      "Garden in May": album("Garden in May", "May · 31 photos", [140, 150, 95, 345, 130, 160, 110, 20]),
      "Birthday dinner": album("Birthday dinner", "April · 22 photos", [40, 20, 60, 350, 30, 80, 10, 55]),
      Scans: album("Scans", "12 pages", [85, 80, 90, 75, 95, 70, 88, 82]),
      "Lisbon in three days": page(`<h3>Lisbon in three days</h3><p class="meta">A first-timer's guide · 6 minute read</p><ul><li>Day 1: Alfama and the castle</li><li>Day 2: Belém, by tram 15</li><li>Day 3: Sintra, by train</li><li>Where to eat by the river</li></ul>`),
      "Flights to Lisbon": page(`<h3>London → Lisbon</h3><p class="meta">Fri 14 Mar · 1 adult</p>${table([["07:40", "10:25", "Direct", "€89"], ["11:15", "14:05", "Direct", "€104"], ["16:50", "19:35", "Direct", "€76"], ["19:20", "23:40", "1 stop", "€61"]])}`),
      "Recipe: green curry": page(`<h3>Thai green curry</h3><p class="meta">35 minutes · serves 4</p><ul><li>3 tbsp green curry paste</li><li>400 ml coconut milk</li><li>500 g chicken thighs</li><li>1 handful Thai basil</li><li>2 tsp fish sauce</li></ul>`),
      "Train times to Sintra": page(`<h3>Rossio → Sintra</h3><p class="meta">Every 20 minutes · 40 min</p>${table([["09:11", "09:51", "Platform 3"], ["09:31", "10:11", "Platform 1"], ["09:51", "10:31", "Platform 3"], ["10:11", "10:51", "Platform 2"]])}`),
      "Casa Alfama — booking": page(`<h3>Casa Alfama</h3><p class="meta">14–18 Mar · 4 nights · ★ 4.9</p><p>A bright two-room flat over the river, five minutes from the tram 28 stop.</p><p class="meta">Free cancellation until 7 Mar</p>`),
      "Weather this week": page(`<h3>Lisbon</h3><p class="meta">This week</p>${table([["Friday", "Sunny", "19°"], ["Saturday", "Sunny", "21°"], ["Sunday", "Some cloud", "18°"], ["Monday", "Showers", "16°"]])}`),
      Inbox: mail(1, "Your stay is confirmed", "Casa Alfama · 09:12", ["Thank you for booking. Your flat is ready from 3 pm on 14 March.", "The keys are in the lock box by the door. We'll send the code the day before."]),
      "Re: Friday dinner": mail(0, "Re: Friday dinner", "Maya Chen · 08:47", ["Sounds good. I'll book the table for eight, at half past seven.", "Can you bring the photos from the trip?"]),
      "Packing for Lisbon": `<div class="main"><h3>Packing list</h3><ul class="check"><li class="done">Passport</li><li class="done">Charger and adapter</li><li>Sunglasses</li><li>Rain jacket</li><li>Book for the flight</li><li>Camera</li></ul></div>`,
      "Gift ideas": `<div class="main"><h3>Gift ideas</h3><ul><li>A book of Lisbon photographs</li><li>An olive-wood board</li><li>Concert tickets</li></ul></div>`,
      Morning: songs("Morning", "12 songs · 46 minutes", [["Slow Tide", "Marta Reis", "3:42"], ["Paper Boats", "The Lanterns", "4:05"], ["Azulejo", "Tiago Sol Trio", "2:58"], ["Window Seat", "Ana Vale", "3:27"]]),
      "Road trip": songs("Road trip", "18 songs · 1 hour 9 minutes", [["Open Road", "The Lanterns", "3:51"], ["Coastline", "Ana Vale", "4:12"], ["Twenty-Eight", "Tiago Sol Trio", "3:08"], ["Last Light", "Marta Reis", "4:40"]]),
    };
    const apps = [
      { name: "Photos", kind: "photos",
        windows: [{ t: "Beach weekend" }, { t: "Garden in May" }, { t: "Birthday dinner" }, { t: "Scans", s: "Other desktop" }] },
      { name: "Browser", kind: "web",
        windows: [{ t: "Lisbon in three days" }, { t: "Flights to Lisbon" }, { t: "Recipe: green curry", s: "Minimized" }, { t: "Train times to Sintra" }, { t: "Casa Alfama — booking" }, { t: "Weather this week" }] },
      { name: "Mail", kind: "mail",
        windows: [{ t: "Inbox" }, { t: "Re: Friday dinner" }] },
      { name: "Notes", kind: "notes",
        windows: [{ t: "Packing for Lisbon" }, { t: "Gift ideas", s: "Hidden" }] },
      { name: "Music", kind: "music",
        windows: [{ t: "Morning" }, { t: "Road trip" }] },
    ];
    desk.insertAdjacentHTML("afterbegin", `<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>${defs.join("")}</defs></svg>`);
    // The features under the demo are pieces of the same panel; their Windows rows get the same icons.
    $$("[data-ic]").forEach((slot) => { slot.innerHTML = fluent[slot.dataset.ic]; });
    let mode = root.dataset.os === "windows" ? "windows" : "mac";
    let app = 1;
    let win = 0;
    let flat = 1;
    let open = true;
    const iconOf = (a) => (mode === "mac" ? glass : fluent)[a.kind];

    const sketch = (a, w) => `<div class="win pv-${a.kind}"><div class="win-bar"><i></i><i></i><i></i><b><span class="win-ic">${fluent[a.kind]}</span>${w.t}</b></div><div class="win-body">${bodies[w.t]}</div></div>`;

    // The desktop around the panel: the menu bar and Dock on a Mac, the taskbar on Windows.
    // The Dock and taskbar keep one order; the menu bar names the app in front.
    const dock = apps.slice();
    const osTop = document.createElement("div");
    const osBottom = document.createElement("div");
    osTop.className = "os-top";
    osBottom.className = "os-bottom";
    osTop.setAttribute("aria-hidden", "true");
    osBottom.setAttribute("aria-hidden", "true");
    panel.before(osTop);
    panel.after(osBottom);
    desk.classList.add("live");
    const search = `<svg viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="6" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M15 15l5 5" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`;
    const paintDesktop = () => {
      const now = new Date();
      const time = now.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
      if (mode === "mac") {
        const day = now.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
        osTop.innerHTML = `<div class="menubar"><b>${apps[0].name}</b>${["File", "Edit", "View", "Window", "Help"].map((m) => `<span>${m}</span>`).join("")}<span class="status"><svg viewBox="0 0 25 16"><use href="#mark"/></svg><span>${day}&ensp;${time}</span></span></div>`;
        osBottom.innerHTML = `<div class="dock">${dock.map((a) => `<span>${iconOf(a)}</span>`).join("")}</div>`;
      } else {
        osTop.innerHTML = "";
        osBottom.innerHTML = `<div class="taskbar"><div class="tb-apps"><span class="tb-search">${search}</span>${dock.map((a) => `<span${a === apps[0] ? ' class="on"' : ""}>${iconOf(a)}</span>`).join("")}</div><div class="tb-tray"><img src="favicon.svg" alt="" width="16" height="16" /><span class="tb-clock"><span>${time}</span><span>${now.toLocaleDateString()}</span></span></div></div>`;
      }
    };
    // Windows lists windows, not apps, in one recent-first order, so apps interleave: it starts
    // as a round of each app's most recent window, then the next round, and so on.
    const recent = [];
    for (let round = 0; recent.length < apps.reduce((n, a) => n + a.windows.length, 0); round += 1) {
      apps.forEach((a) => { if (a.windows[round]) recent.push(a.windows[round]); });
    }
    const owner = (w) => apps.find((a) => a.windows.includes(w));
    const all = () => recent.map((w) => { const a = owner(w); return { a, ai: apps.indexOf(a), w, wi: a.windows.indexOf(w) }; });
    const rows = () => (mode === "mac" ? apps[app].windows.map((w, wi) => ({ a: apps[app], ai: app, w, wi })) : all().slice(0, 7));
    const picked = () => (mode === "mac" ? win : flat);

    // The window in front changes only on a switch; the new one is laid over the old and fades in.
    let shown = null;
    const paintFront = () => {
      const top = apps[0].windows[0];
      if (top === shown) return;
      shown = top;
      front.insertAdjacentHTML("beforeend", sketch(apps[0], top));
      while (front.children.length > 2) front.firstElementChild.remove();
    };
    const render = () => {
      desk.dataset.mode = mode;
      const list = rows();
      const pick = picked();
      const strip = apps.map((a, ai) => `<button class="tile" type="button" tabindex="-1" data-app="${ai}" aria-pressed="${ai === app}" aria-label="${a.name}">${iconOf(a)}</button>`).join("");
      const items = list.map((r, i) => `<li><button class="row" type="button" tabindex="-1" data-row="${i}" aria-current="${i === pick}"><span class="n">${i + 1}</span><span class="ic">${iconOf(r.a)}</span><span class="t">${r.w.t}<small>${r.a.name}</small></span>${r.w.s ? `<span class="s">${r.w.s}</span>` : ""}</button></li>`).join("");
      const chosen = list[pick];
      panel.innerHTML = `<div class="sw-strip">${strip}</div><div class="sw-name"><span>${apps[app].name}</span></div><div class="sw-body"><ol class="sw-list" aria-label="Windows">${items}</ol><div class="sw-well" aria-hidden="true">${sketch(chosen.a, chosen.w)}</div></div>`;
      paintFront();
      paintDesktop();
      $$("[data-os-pick]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.osPick === mode)));
    };

    // A switch: the window goes to the front of its app, and the app to the front of the list.
    const go = (ai, wi) => {
      const [a] = apps.splice(ai, 1);
      const [w] = a.windows.splice(wi, 1);
      a.windows.unshift(w);
      apps.unshift(a);
      recent.splice(recent.indexOf(w), 1);
      recent.unshift(w);
      open = false;
      desk.classList.add("closed", "switched");
      reopen.hidden = false;
      // The panel keeps its list while it fades out; it is redrawn when it opens again.
      paintFront();
      paintDesktop();
      status.textContent = `Switched to ${w.t} in ${a.name}.`;
      // Focus stays with the demo, so the next key brings the switcher back.
      panel.focus({ preventScroll: true });
    };
    // It opens where the system switcher does: on the app or window used before this one.
    const show = (focus = true) => {
      open = true;
      app = 1;
      win = 0;
      flat = 1;
      desk.classList.remove("closed", "switched");
      reopen.hidden = true;
      render();
      if (focus) panel.focus({ preventScroll: true });
    };
    const choose = (i) => {
      const r = rows()[i];
      if (r) go(r.ai, r.wi);
    };
    const step = (key) => {
      if (mode === "mac" && (key === "ArrowLeft" || key === "ArrowRight")) {
        app = (app + (key === "ArrowRight" ? 1 : apps.length - 1)) % apps.length;
        win = 0;
      } else if (key === "ArrowUp" || key === "ArrowDown") {
        const n = rows().length;
        const next = (picked() + (key === "ArrowDown" ? 1 : n - 1)) % n;
        if (mode === "mac") win = next;
        else flat = next;
      } else return;
      render();
    };

    const light = (key) => {
      const caps = $$(`kbd[data-k="${/^[1-9]$/.test(key) ? "digit" : key}"]`, keys);
      caps.forEach((k) => k.setAttribute("data-down", ""));
      setTimeout(() => caps.forEach((k) => k.removeAttribute("data-down")), 160);
    };
    // In view: at least half the desk is on screen.
    const inView = () => {
      const r = desk.getBoundingClientRect();
      return Math.min(r.bottom, innerHeight) - Math.max(r.top, 0) > r.height / 2;
    };
    document.addEventListener("keydown", (e) => {
      if (e.metaKey || e.ctrlKey || e.altKey || e.defaultPrevented) return;
      const inPanel = panel.contains(e.target);
      const idle = e.target === document.body || desk.contains(e.target);
      const digit = /^[1-9]$/.test(e.key);
      const arrow = /^Arrow(Up|Down|Left|Right)$/.test(e.key);
      // Digits work while the demo is in view; arrows and Enter only once it has focus,
      // so they keep scrolling the page otherwise.
      if (digit ? !(inPanel || (idle && inView())) : !(inPanel && (arrow || e.key === "Enter"))) return;
      e.preventDefault();
      light(e.key);
      if (!open) show();
      else if (digit) choose(Number(e.key) - 1);
      else if (arrow) step(e.key);
      else choose(picked());
    });
    panel.addEventListener("click", (e) => {
      const tile = e.target.closest("[data-app]");
      const row = e.target.closest("[data-row]");
      if (tile) {
        app = Number(tile.dataset.app);
        win = 0;
        render();
        panel.focus({ preventScroll: true });
      } else if (row) choose(Number(row.dataset.row));
    });
    // Pointing at a row selects it, as hovering does in the real panel.
    panel.addEventListener("pointermove", (e) => {
      const row = e.target.closest("[data-row]");
      if (!row || e.pointerType !== "mouse" || Number(row.dataset.row) === picked()) return;
      if (mode === "mac") win = Number(row.dataset.row);
      else flat = Number(row.dataset.row);
      render();
    });
    reopen.addEventListener("click", () => show());
    let swapping = 0;
    $$("[data-os-pick]").forEach((b) => b.addEventListener("click", () => {
      if (mode === b.dataset.osPick && !desk.classList.contains("swapping")) return;
      // The whole desktop cross-fades into the other system's, so nothing in it jumps; the
      // install buttons above change with it.
      if (document.startViewTransition) {
        root.classList.add("demo-swap");
        const t = document.startViewTransition(() => {
          mode = b.dataset.osPick;
          // On a computer, the install buttons follow the system the demo shows.
          if (root.dataset.os !== "other") root.dataset.os = mode;
          root.dataset.demo = mode;
          // The keys drive the demo straight after the switch, as they did before it.
          show();
        });
        t.finished.finally(() => root.classList.toggle("demo-swap", false));
        return;
      }
      $$("[data-os-pick]").forEach((p) => p.setAttribute("aria-pressed", String(p === b)));
      // Elsewhere the panel fades out, changes system, and fades back in; the last pick wins.
      clearTimeout(swapping);
      desk.classList.add("swapping");
      swapping = setTimeout(() => {
        desk.classList.remove("swapping");
        if (mode === b.dataset.osPick) return;
        mode = b.dataset.osPick;
        // On a computer, the install buttons follow the system the demo shows.
        if (root.dataset.os !== "other") root.dataset.os = mode;
        root.dataset.demo = mode;
        // The keys drive the demo straight after the switch, as they did before it.
        show();
      }, 100);
    }));
    render();
  }

  /* The first feature answers as the switcher does: point at a row, or press its number once the
     demo above has scrolled away, and that row is selected by the key shown over the list. */
  const picks = $$(".fx-pick");
  const pick = (box, i, pressed) => {
    const rows = $$(".row", box);
    if (!rows[i]) return;
    rows.forEach((row, k) => (k === i ? row.setAttribute("aria-current", "true") : row.removeAttribute("aria-current")));
    const key = $(".fx-key", box);
    key.textContent = i + 1;
    if (pressed && key.animate && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
      key.animate([{ translate: "0 0.14em" }, { translate: "0 0" }], { duration: 160, easing: "cubic-bezier(0.23, 1, 0.32, 1)" });
    }
  };
  picks.forEach((box) => box.addEventListener("pointerover", (e) => {
    const row = e.target.closest(".row");
    if (row && e.pointerType === "mouse") pick(box, $$(".row", box).indexOf(row), false);
  }));
  document.addEventListener("keydown", (e) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.target !== document.body || !/^[1-9]$/.test(e.key)) return;
    const box = picks.find((b) => getComputedStyle(b).visibility === "visible");
    if (!box) return;
    const r = box.getBoundingClientRect();
    if (r.top >= 0 && r.bottom <= innerHeight) pick(box, Number(e.key) - 1, true);
  });

  /* ─────────────────────────────────────────────────────────
   * THE CLOSING KEY
   *
   * The key is the app's icon. Pressing it steps through the icon
   * colours the app offers in Settings, in the app's order.
   *
   *    0ms   finger down: the cap sinks 44 units into its foot
   *          and its glow draws in (70ms)
   *  release the colour changes while the cap is at the bottom
   *          (160ms), and the cap springs back past its resting
   *          height and settles (460ms); the glow lets out
   *
   * A quick tap still holds the cap down for `hold`, so the press
   * is always seen. The real Tab key presses it too while it is on
   * screen, and Shift steps back, as Shift+Tab does in the switcher.
   * Tab still moves focus as usual.
   * ───────────────────────────────────────────────────────── */
  const KEY = {
    hold: 110, // ms the cap stays down at the least
    // Settings > Icon colour, as COLORS in scripts/build-icons.py.
    colours: { azure: "#3297e9", copper: "#c27e49", ember: "#f47016", indigo: "#5679f3", orchid: "#dd49af", rosewood: "#e43d60", vermilion: "#ec4b22", violet: "#8c5ff0" },
  };
  const key = $(".closing-key");
  if (key) {
    const names = Object.keys(KEY.colours);
    const said = $("#key-colour");
    // Towards black by a factor, or towards white by a share: shade() and tint() in the icon script.
    const blend = (hex, f, white) => "#" + [1, 3, 5].map((i) => {
      const v = parseInt(hex.slice(i, i + 2), 16);
      return Math.round(white ? v + (255 - v) * f : v * f).toString(16).padStart(2, "0");
    }).join("");
    // Skirt top and bottom, face top and bottom, legend: palette() in the icon script.
    const palette = (name) => {
      const c = KEY.colours[name];
      return name === "azure" ? ["#2f6fb8", "#1f4f8c", "#4aa6f0", "#2f84d4", "#f0f8ff"]
        : [blend(c, 0.68), blend(c, 0.5), blend(c, 0.12, true), blend(c, 0.86), blend(c, 0.94, true)];
    };
    let at = 0;
    let downAt = 0;
    let timer = 0;
    const press = () => {
      clearTimeout(timer);
      if (!key.classList.contains("down")) downAt = performance.now();
      key.classList.add("down");
    };
    // Lets the cap up after its hold; a step other than 0 changes the colour first.
    const release = (step) => {
      if (step) {
        at = (at + step + names.length) % names.length;
        const [skirtTop, skirtBottom, faceTop, faceBottom, ink] = palette(names[at]);
        Object.entries({ "--key": KEY.colours[names[at]], "--key-skirt-top": skirtTop, "--key-skirt-bottom": skirtBottom, "--key-face-top": faceTop, "--key-face-bottom": faceBottom, "--key-ink": ink })
          .forEach(([name, value]) => key.style.setProperty(name, value));
        if (said) said.textContent = `Icon colour: ${names[at]}`;
      }
      clearTimeout(timer);
      timer = setTimeout(() => key.classList.remove("down"), Math.max(0, KEY.hold - (performance.now() - downAt)));
    };
    key.addEventListener("pointerdown", press);
    key.addEventListener("pointerleave", () => { if (key.classList.contains("down")) release(0); });
    key.addEventListener("pointercancel", () => release(0));
    // A click is the finger letting go on the key, or Enter or Space with the key in focus.
    key.addEventListener("click", (e) => { press(); release(e.shiftKey ? -1 : 1); });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Tab" || e.repeat || e.metaKey || e.ctrlKey || e.altKey) return;
      const r = key.getBoundingClientRect();
      if (r.bottom < 0 || r.top > innerHeight) return;
      press();
      release(e.shiftKey ? -1 : 1);
    });
  }

})();
