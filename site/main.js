// PilotHouse's site: the radar under the hero, scroll-driven motion (and its
// fallback where the browser has no `animation-timeline`), the live Agents
// panel and chat box, the pinned tour, the sideways workbench, the stacking
// Studio windows, the day's timeline, the theme painter, and the download
// for this computer. Everything still reads with motion turned off.

const root = document.documentElement;
root.classList.add("js");
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const $ = (selector, scope = document) => scope.querySelector(selector);
const $$ = (selector, scope = document) => [...scope.querySelectorAll(selector)];
const clamp = (value, low = 0, high = 1) => Math.min(high, Math.max(low, value));
const supports = (rule) => window.CSS && CSS.supports(rule);
const navHeight = () => parseFloat(getComputedStyle(root).getPropertyValue("--nav-h")) || 64;

// ---- One loop for everything that follows the scroll position ------------
const onFrame = new Set();
const onResize = new Set();
let queued = false;
const schedule = () => {
  if (queued) return;
  queued = true;
  requestAnimationFrame(() => {
    queued = false;
    for (const run of onFrame) run();
  });
};
addEventListener("scroll", schedule, { passive: true });
addEventListener("resize", () => {
  for (const run of onResize) run();
  schedule();
});
addEventListener("load", () => {
  for (const run of onResize) run();
  schedule();
});

// Runs `start` while `element` is on screen and `stop` when it leaves.
const whileVisible = (element, start, stop, margin = "0px") => {
  if (!element) return;
  new IntersectionObserver(([entry]) => (entry.isIntersecting ? start() : stop()), { rootMargin: margin }).observe(element);
};

// ---- Scroll progress, where CSS cannot follow the page itself -------------
if (!supports("animation-timeline: scroll()")) {
  const bar = $(".progress");
  onFrame.add(() => {
    const max = root.scrollHeight - innerHeight;
    bar.style.setProperty("--p", max > 0 ? (scrollY / max).toFixed(4) : 0);
  });
}

// ---- Scroll-driven effects, scrubbed by hand without `animation-timeline` --
// Each `.sd` element plays between --a and --b of the time it crosses the
// screen (as `animation-range: cover --a cover --b` would). Positions are
// read from the layout (offsets), not from the moving element.
if (!reduceMotion && !supports("animation-timeline: view()")) {
  root.classList.add("no-sda");
  const top = (node) => {
    let y = 0;
    for (; node; node = node.offsetParent) y += node.offsetTop;
    return y;
  };
  const items = $$(".sd").map((element) => {
    const style = getComputedStyle(element);
    // An SVG mark follows the page it is drawn on.
    const box = element instanceof SVGElement ? element.closest("svg").parentElement : element;
    return {
      element,
      box,
      a: (parseFloat(style.getPropertyValue("--a")) || 0) / 100,
      b: (parseFloat(style.getPropertyValue("--b")) || 100) / 100,
      y: 0,
      h: 0,
    };
  });
  const measure = () => {
    for (const item of items) {
      item.y = top(item.box);
      item.h = item.box.offsetHeight;
    }
  };
  measure();
  onResize.add(measure);
  onFrame.add(() => {
    const height = innerHeight;
    for (const item of items) {
      const at = item.y - scrollY;
      if (at > height * 1.5 || at + item.h < -height * 0.5) continue;
      const cover = (height - at) / (height + item.h);
      const p = clamp((cover - item.a) / Math.max(0.0001, item.b - item.a));
      item.element.style.setProperty("--p", p.toFixed(4));
    }
  });
}

// ---- The headline rises word by word ------------------------------------
$$(".headline .w").forEach((word, i) => word.style.setProperty("--i", i));

// ---- The radar only turns while the hero shows -----------------------------
const hero = $(".hero");
whileVisible(hero, () => hero.classList.remove("paused"), () => hero.classList.add("paused"));

// ---- Text arrives as it scrolls in ---------------------------------------
$$(".kinds li").forEach((kind, i) => kind.style.setProperty("--k", i));
const reveal = new IntersectionObserver(
  (entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add("shown");
      reveal.unobserve(entry.target);
    }
  },
  { rootMargin: "0px 0px -12% 0px" }
);
$$(".reveal, .race, .kinds").forEach((element) => reveal.observe(element));

// ---- The section in view, marked in the bar -------------------------------
(() => {
  const links = new Map($$(".nav nav a").map((link) => [link.getAttribute("href").slice(1), link]));
  // Every section, so one the bar does not name clears the mark.
  const watched = $$("main > section");
  const seen = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) links.forEach((link, id) => link.classList.toggle("on", id === entry.target.id));
      }
    },
    { rootMargin: "-45% 0px -50% 0px" }
  );
  watched.forEach((section) => seen.observe(section));
})();

// ---- The Agents panel, alive --------------------------------------------
(() => {
  const lists = {
    needs: $('[data-list="needs"]'),
    working: $('[data-list="working"]'),
    done: $('[data-list="done"]'),
  };
  const counts = {
    needs: $('[data-sec="needs"] .count'),
    working: $('[data-sec="working"] .count'),
    done: $('[data-sec="done"] .count'),
  };
  const workspaces = {
    harbor: "#f472b6",
    "tidewatch-android": "#5eead4",
    "harbour-office": "#a78bfa",
    "docs-site": "#fbbf24",
  };
  const row = (state, chat) => {
    const element = document.createElement("div");
    element.className = `ag ${state}`;
    const glyph = document.createElement("span");
    glyph.className = state === "needs" ? "glyph q" : state === "working" ? "glyph spin" : "glyph done";
    if (state === "needs") glyph.textContent = "?";
    const title = document.createElement("span");
    title.className = "t";
    title.textContent = chat.title;
    const age = document.createElement("span");
    age.className = "age";
    age.textContent = chat.age;
    const meta = document.createElement("span");
    meta.className = "meta";
    const dot = document.createElement("i");
    dot.style.setProperty("--c", workspaces[chat.workspace]);
    meta.append(dot, `${chat.workspace} · ${chat.agent}`);
    const line = document.createElement("span");
    line.className = "line";
    line.textContent = state === "needs" ? chat.ask : state === "working" ? chat.doing : chat.said;
    element.append(glyph, title, age, meta, line);
    return element;
  };
  const chats = [
    { title: "Build the release and deploy it to staging", workspace: "tidewatch-android", agent: "Codex", ask: "./scripts/deploy.sh staging", doing: "Building the release", said: "Deployed to staging.", age: "3m" },
    { title: "Start the arrivals board for the office screen", workspace: "harbour-office", agent: "Antigravity", ask: "Write src/board.rs?", doing: "Laying out today's arrivals", said: "The board shows boats due today.", age: "2m" },
    { title: "Add a rate limit to bookings", workspace: "harbor", agent: "Claude Code", ask: "Run cargo test?", doing: "Running the tests", said: "Bookings are limited to 30 a minute.", age: "4m" },
    { title: "Fix the broken links in the guide", workspace: "docs-site", agent: "Codex", ask: "Delete old/guide.md?", doing: "Checking 214 links", said: "Fixed 6 links; all pages build.", age: "1m" },
  ];
  const state = ["needs", "working", "working", "done"];
  const render = () => {
    for (const key of Object.keys(lists)) lists[key].replaceChildren();
    chats.forEach((chat, i) => lists[state[i]].append(row(state[i], chat)));
    for (const key of Object.keys(counts)) counts[key].textContent = state.filter((s) => s === key).length;
  };
  render();
  if (reduceMotion) return;
  const next = { working: "done", done: "working", needs: "working" };
  let turn = 0;
  let timer = 0;
  // One chat moves on each beat: a question answered, a task finished, a
  // new one started, and now and then an agent asks something.
  const beat = () => {
    turn++;
    const i = turn % chats.length;
    if (turn % 5 === 0) state[i] = "needs";
    else state[i] = next[state[i]];
    if (!state.includes("needs")) state[(i + 1) % chats.length] = "needs";
    render();
  };
  whileVisible(
    $(".panel-demo"),
    () => (timer = timer || setInterval(beat, 2600)),
    () => {
      clearInterval(timer);
      timer = 0;
    }
  );
})();

// ---- Typing an @ mention ------------------------------------------------
(() => {
  const text = $(".composer .text");
  const suggest = $(".suggest");
  const items = $$(".sugg", suggest);
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const mention = (label) => {
    const span = document.createElement("span");
    span.className = "mention";
    span.textContent = label;
    return span;
  };
  if (reduceMotion) {
    text.append("Why does ", mention("@src/routes/bookings.rs"), " return 409 here? ", mention("@screenshot"));
    return;
  }
  let visible = false;
  const type = async (chars, speed = 55) => {
    for (const char of chars) {
      text.append(char);
      await wait(speed + Math.random() * 40);
    }
  };
  const highlight = (index) => items.forEach((item, i) => item.classList.toggle("hi", i === index));
  const run = async () => {
    for (;;) {
      // Paused off screen, between rounds.
      while (!visible) await wait(400);
      text.replaceChildren();
      await wait(700);
      await type("Why does ");
      await type("@", 120);
      suggest.classList.add("open");
      for (const index of [0, 1, 2, 1]) {
        highlight(index);
        await wait(420);
      }
      await type("rou", 110);
      suggest.classList.remove("open");
      // The characters typed after "Why does " become the mention.
      [...text.childNodes].slice(-4).forEach((node) => node.remove());
      text.append(mention("@src/routes/bookings.rs"));
      await type(" return 409 here? ");
      await type("@", 120);
      highlight(3);
      suggest.classList.add("open");
      await wait(900);
      suggest.classList.remove("open");
      text.lastChild?.remove();
      text.append(mention("@screenshot"));
      await wait(2600);
    }
  };
  let started = false;
  whileVisible(
    $(".composer-demo"),
    () => {
      visible = true;
      if (!started) {
        started = true;
        run();
      }
    },
    () => (visible = false)
  );
})();

// ---- The tour: the step in front zooms the pinned window ------------------
(() => {
  const layers = Object.fromEntries($$(".tour-window .layer").map((layer) => [layer.dataset.layer, layer]));
  const spotlight = $(".spotlight");
  const dots = $$(".tour-dots i");
  const steps = $$(".step");
  let current = null;
  const show = (step) => {
    current = step;
    steps.forEach((other, i) => {
      other.classList.toggle("on", other === step);
      dots[i]?.classList.toggle("on", other === step);
    });
    for (const [key, layer] of Object.entries(layers)) {
      const on = key === step.dataset.layer;
      layer.classList.toggle("on", on);
      if (on) {
        layer.style.setProperty("--x", `${-Number(step.dataset.x)}%`);
        layer.style.setProperty("--y", `${-Number(step.dataset.y)}%`);
        layer.style.setProperty("--s", step.dataset.s);
      }
    }
    spotlight.style.setProperty("--spot", step.dataset.spot);
  };
  // The step in front is the last one whose top has passed the middle of
  // the screen (from the scroll position, so a fast scroll skips none).
  onFrame.add(() => {
    const line = innerHeight * 0.55;
    const step = steps.filter((each) => each.getBoundingClientRect().top < line).pop() || steps[0];
    if (step !== current) show(step);
  });
  show(steps[0]);
})();

// ---- The workbench: down the page moves the strip sideways ----------------
(() => {
  const section = $(".hs");
  const track = $(".hs-track");
  if (!section || reduceMotion) return;
  root.classList.add("hs-on");
  let distance = 0;
  const measure = () => {
    distance = Math.max(0, track.scrollWidth - innerWidth);
    // As much page to scroll as the strip has to move, and a little to rest.
    section.style.height = `${distance + innerHeight + innerHeight * 0.25}px`;
  };
  measure();
  onResize.add(measure);
  onFrame.add(() => {
    const box = section.getBoundingClientRect();
    if (box.bottom < 0 || box.top > innerHeight) return;
    const pinned = innerHeight - navHeight();
    const p = clamp((navHeight() - box.top) / Math.max(1, box.height - pinned));
    track.style.setProperty("--hs-x", `${(-p * distance).toFixed(1)}px`);
  });
})();

// ---- Studio: a window steps back as the next one covers it ----------------
(() => {
  const cards = $$(".stack-card");
  if (reduceMotion || cards.length < 2) return;
  let stops = [];
  const measure = () => (stops = cards.map((card) => parseFloat(getComputedStyle(card).top) || 0));
  measure();
  onResize.add(measure);
  onFrame.add(() => {
    for (let i = 0; i < cards.length - 1; i++) {
      const next = cards[i + 1].getBoundingClientRect().top;
      const p = clamp((innerHeight - next) / Math.max(1, innerHeight - stops[i + 1]));
      cards[i].style.setProperty("--shrink", (1 - p * 0.06).toFixed(4));
      cards[i].style.setProperty("--dimmed", (1 - p * 0.5).toFixed(4));
    }
  });
})();

// ---- File kinds light up in turn as the page passes ------------------------
(() => {
  const section = $(".previews");
  const kinds = $$(".kinds li");
  if (!section || reduceMotion) return;
  let lit = -1;
  onFrame.add(() => {
    const box = section.getBoundingClientRect();
    if (box.bottom < 0 || box.top > innerHeight) return;
    const p = clamp((innerHeight - box.top) / (innerHeight + box.height));
    const index = Math.floor(p * kinds.length * 1.6) % kinds.length;
    if (index === lit) return;
    kinds[lit]?.classList.remove("lit");
    kinds[index].classList.add("lit");
    lit = index;
  });
})();

// ---- The day's line draws down as you read ------------------------------
(() => {
  const line = $(".timeline");
  const items = $$(".timeline li");
  if (!line) return;
  if (reduceMotion) {
    items.forEach((item) => item.classList.add("lit"));
    return;
  }
  line.style.setProperty("--fill", 0);
  onFrame.add(() => {
    const box = line.getBoundingClientRect();
    if (box.bottom < -200 || box.top > innerHeight + 200) return;
    const mark = innerHeight * 0.62;
    line.style.setProperty("--fill", clamp((mark - box.top) / box.height).toFixed(4));
    for (const item of items) item.classList.toggle("lit", item.getBoundingClientRect().top + 12 < mark);
  });
})();

// ---- Themes: the window repaints from the swatch picked -----------------
(() => {
  const section = $(".themes");
  const frame = $(".themes-window");
  const swatches = $$(".swatch");
  const GLOW = {
    "modern-dark": "rgba(0, 120, 212, 0.6)",
    "modern-light": "rgba(156, 195, 255, 0.45)",
    "warm-dark": "rgba(217, 164, 91, 0.5)",
    "warm-light": "rgba(232, 201, 160, 0.4)",
    dracula: "rgba(189, 147, 249, 0.55)",
    "one-dark": "rgba(77, 120, 204, 0.55)",
    "one-light": "rgba(82, 111, 255, 0.45)",
    "studio-dark": "rgba(53, 116, 240, 0.55)",
  };
  const source = (name) => `shots/themes/${name}.webp`;
  let current = swatches[0].dataset.theme;
  const paint = (swatch) => {
    const name = swatch.dataset.theme;
    if (name === current) return;
    current = name;
    swatches.forEach((other) => {
      other.setAttribute("aria-checked", String(other === swatch));
      other.tabIndex = other === swatch ? 0 : -1;
    });
    frame.style.setProperty("--glow", GLOW[name]);
    // The paint spreads from the swatch, wherever it is next to the window.
    const box = frame.getBoundingClientRect();
    const from = swatch.getBoundingClientRect();
    const next = document.createElement("img");
    next.className = reduceMotion ? "paint" : "paint wet";
    next.width = 1600;
    next.height = 1000;
    next.alt = `PilotHouse in the ${swatch.textContent} theme, with Claude Code's chat in the harbor workspace.`;
    next.style.setProperty("--cx", `${((from.left + from.width / 2 - box.left) / box.width) * 100}%`);
    next.style.setProperty("--cy", `${((from.top + from.height / 2 - box.top) / box.height) * 100}%`);
    next.src = source(name);
    // Only the pictures under this one go, so a quick second pick keeps its own.
    const settle = () => {
      while (next.previousElementSibling) next.previousElementSibling.remove();
      next.classList.remove("wet");
    };
    next
      .decode()
      .catch(() => {})
      .then(() => {
        frame.append(next);
        if (reduceMotion) settle();
        else next.addEventListener("animationend", settle, { once: true });
      });
  };
  let cycling = !reduceMotion;
  swatches.forEach((swatch, i) => {
    swatch.tabIndex = i === 0 ? 0 : -1;
    swatch.addEventListener("click", () => {
      cycling = false;
      paint(swatch);
    });
    swatch.addEventListener("keydown", (event) => {
      const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
      if (!step) return;
      event.preventDefault();
      cycling = false;
      const target = swatches[(i + step + swatches.length) % swatches.length];
      target.focus();
      paint(target);
    });
  });
  // On screen: fetch every theme's picture, and show them in turn until a
  // swatch is picked.
  let timer = 0;
  let fetched = false;
  whileVisible(
    section,
    () => {
      if (!fetched) {
        fetched = true;
        swatches.forEach((swatch) => (new Image().src = source(swatch.dataset.theme)));
      }
      clearInterval(timer);
      timer = setInterval(() => {
        if (!cycling) return clearInterval(timer);
        const at = swatches.findIndex((swatch) => swatch.dataset.theme === current);
        paint(swatches[(at + 1) % swatches.length]);
      }, 3200);
    },
    () => clearInterval(timer)
  );
})();

// ---- Numbers count up ---------------------------------------------------
(() => {
  const count = (element) => {
    const end = Number(element.dataset.count);
    const prefix = element.dataset.prefix || "";
    const suffix = element.dataset.suffix || "";
    if (reduceMotion || end === 0) {
      element.textContent = `${prefix}${end}${suffix}`;
      return;
    }
    const start = performance.now();
    const step = (now) => {
      const t = Math.min((now - start) / 1400, 1);
      const eased = 1 - Math.pow(1 - t, 3);
      element.textContent = `${prefix}${Math.round(end * eased)}${suffix}`;
      if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  };
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      count(entry.target);
      observer.unobserve(entry.target);
    }
  });
  $$("[data-count]").forEach((element) => observer.observe(element));
})();

// ---- The download for this computer -------------------------------------
(() => {
  const platform = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
  const os = platform.includes("mac")
    ? "mac"
    : platform.includes("win")
      ? "windows"
      : platform.includes("linux") && !/android/i.test(navigator.userAgent)
        ? "linux"
        : null;
  if (!os) return;
  const card = $(`.dl[data-os="${os}"]`);
  const main = $("#download-main");
  if (card && main) {
    card.classList.add("mine");
    main.textContent = `Download for ${card.querySelector(".os").textContent}`;
    main.href = card.href;
  }
  $$(`.mirrors a[data-os="${os}"]`).forEach((link) => link.classList.add("mine"));
})();

schedule();
