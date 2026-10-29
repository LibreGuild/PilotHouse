// PilotHouse's site: the sea under the hero, the pointer's glow and pull,
// the live Agents panel and chat box, the scroll tour, and the download
// for this computer. Everything still reads with motion turned off.

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const finePointer = window.matchMedia("(pointer: fine)").matches;
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

// ---- Scroll progress ----------------------------------------------------
const progress = $(".progress");
const onScroll = () => {
  const max = document.documentElement.scrollHeight - innerHeight;
  progress.style.setProperty("--p", max > 0 ? scrollY / max : 0);
};
addEventListener("scroll", onScroll, { passive: true });
onScroll();

// ---- The headline rises word by word ------------------------------------
$$(".headline .w").forEach((word, i) => word.style.setProperty("--i", i));
requestAnimationFrame(() => $(".headline").classList.add("in"));

// ---- Reveal on scroll ---------------------------------------------------
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
$$(".reveal, .race").forEach((element) => reveal.observe(element));

// ---- The pointer's glow -------------------------------------------------
const glow = $(".glow-cursor");
const pointer = { x: innerWidth / 2, y: innerHeight / 3 };
if (finePointer && !reduceMotion) {
  const at = { ...pointer };
  addEventListener("pointermove", (event) => {
    pointer.x = event.clientX;
    pointer.y = event.clientY;
  });
  const follow = () => {
    at.x += (pointer.x - at.x) * 0.12;
    at.y += (pointer.y - at.y) * 0.12;
    glow.style.setProperty("--gx", `${at.x}px`);
    glow.style.setProperty("--gy", `${at.y}px`);
    requestAnimationFrame(follow);
  };
  follow();
} else {
  glow.remove();
}

// ---- Magnetic buttons: they lean toward the pointer ---------------------
if (finePointer && !reduceMotion) {
  for (const button of $$(".magnetic")) {
    button.addEventListener("pointermove", (event) => {
      const box = button.getBoundingClientRect();
      const dx = event.clientX - (box.left + box.width / 2);
      const dy = event.clientY - (box.top + box.height / 2);
      button.style.transform = `translate(${dx * 0.22}px, ${dy * 0.3}px)`;
    });
    button.addEventListener("pointerleave", () => (button.style.transform = ""));
  }
}

// ---- The app tilts with the pointer -------------------------------------
const hero = $(".hero");
const tilt = $(".tilt");
const windowShot = $(".hero-stage .window");
if (finePointer && !reduceMotion) {
  hero.addEventListener("pointermove", (event) => {
    const x = event.clientX / innerWidth - 0.5;
    const y = event.clientY / innerHeight - 0.5;
    tilt.style.setProperty("--ry", `${x * 10}deg`);
    tilt.style.setProperty("--rx", `${8 - y * 8}deg`);
    windowShot.style.setProperty("--shine", `${120 - (x + 0.5) * 140}%`);
  });
  hero.addEventListener("pointerleave", () => {
    tilt.style.setProperty("--ry", "0deg");
    tilt.style.setProperty("--rx", "8deg");
  });
  // Flattens as it scrolls into view.
  addEventListener(
    "scroll",
    () => {
      const flat = Math.min(scrollY / 500, 1);
      tilt.style.setProperty("--rx", `${8 * (1 - flat)}deg`);
    },
    { passive: true }
  );
}

// ---- The sea: lines of light that ripple around the pointer -------------
(() => {
  const canvas = $(".sea");
  const context = canvas.getContext("2d");
  let width = 0;
  let height = 0;
  let scale = 1;
  let stars = [];
  const resize = () => {
    scale = Math.min(devicePixelRatio || 1, 2);
    width = canvas.clientWidth;
    height = canvas.clientHeight;
    canvas.width = width * scale;
    canvas.height = height * scale;
    context.setTransform(scale, 0, 0, scale, 0, 0);
    stars = Array.from({ length: Math.round((width * height) / 9000) }, () => ({
      x: Math.random() * width,
      y: Math.random() * height * 0.5,
      r: Math.random() * 1.2 + 0.2,
      phase: Math.random() * Math.PI * 2,
    }));
  };
  resize();
  addEventListener("resize", resize);

  const local = { x: -9999, y: -9999 };
  hero.addEventListener("pointermove", (event) => {
    const box = canvas.getBoundingClientRect();
    local.x = event.clientX - box.left;
    local.y = event.clientY - box.top;
  });
  hero.addEventListener("pointerleave", () => {
    local.x = local.y = -9999;
  });

  const LINES = 34;
  const draw = (time) => {
    const t = time / 1000;
    context.clearRect(0, 0, width, height);
    for (const star of stars) {
      const twinkle = 0.35 + 0.65 * Math.abs(Math.sin(t * 0.8 + star.phase));
      context.fillStyle = `rgba(226, 220, 255, ${0.55 * twinkle})`;
      context.beginPath();
      context.arc(star.x, star.y, star.r, 0, Math.PI * 2);
      context.fill();
    }
    const horizon = height * 0.5;
    for (let i = 0; i < LINES; i++) {
      // Far lines bunch up near the horizon, near ones spread out.
      const depth = i / (LINES - 1);
      const base = horizon + Math.pow(depth, 1.7) * (height - horizon + 40);
      const amplitude = 3 + depth * 16;
      const alpha = 0.06 + depth * 0.32;
      const hue = 270 - depth * 80; // violet far, cyan near
      context.strokeStyle = `hsla(${hue}, 90%, ${68 + depth * 8}%, ${alpha})`;
      context.lineWidth = 0.6 + depth * 1.2;
      context.beginPath();
      for (let x = -20; x <= width + 20; x += 14) {
        let y =
          base +
          Math.sin(x * (0.006 - depth * 0.002) + t * (0.6 + depth * 0.5) + i * 0.7) * amplitude +
          Math.sin(x * 0.017 - t * 1.1 + i) * amplitude * 0.25;
        // A ripple around the pointer.
        const dx = x - local.x;
        const dy = base - local.y;
        const distance = Math.sqrt(dx * dx + dy * dy);
        if (distance < 260) {
          const fall = 1 - distance / 260;
          y += Math.sin(distance * 0.06 - t * 5) * 14 * fall * fall * (0.4 + depth);
        }
        if (x === -20) context.moveTo(x, y);
        else context.lineTo(x, y);
      }
      context.stroke();
    }
  };

  if (reduceMotion) {
    draw(0);
    return;
  }
  let running = false;
  let frame = 0;
  const loop = (time) => {
    draw(time);
    if (running) frame = requestAnimationFrame(loop);
  };
  new IntersectionObserver(([entry]) => {
    running = entry.isIntersecting;
    cancelAnimationFrame(frame);
    if (running) frame = requestAnimationFrame(loop);
  }).observe(hero);
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
    const glyph = state === "needs" ? '<span class="glyph q">?</span>' : state === "working" ? '<span class="glyph spin"></span>' : '<span class="glyph done"></span>';
    const line = state === "needs" ? chat.ask : state === "working" ? chat.doing : chat.said;
    element.innerHTML = `${glyph}<span class="t"></span><span class="age"></span>
      <span class="meta"><i style="--c:${workspaces[chat.workspace]}"></i>${chat.workspace} · ${chat.agent}</span>
      <span class="line"></span>`;
    element.querySelector(".t").textContent = chat.title;
    element.querySelector(".age").textContent = chat.age;
    element.querySelector(".line").textContent = line;
    return element;
  };
  const chats = [
    { title: "Build the release and deploy it to staging", workspace: "tidewatch-android", agent: "Codex", ask: "./scripts/deploy.sh staging", doing: "Building the release", said: "Deployed to staging.", age: "3m" },
    { title: "Start the arrivals board for the office screen", workspace: "harbour-office", agent: "Gemini CLI", ask: "Write src/board.rs?", doing: "Laying out today's arrivals", said: "The board shows boats due today.", age: "2m" },
    { title: "Add a rate limit to bookings", workspace: "harbor", agent: "Claude Code", ask: "Run cargo test?", doing: "Running the tests", said: "Bookings are limited to 30 a minute.", age: "4m" },
    { title: "Fix the broken links in the guide", workspace: "docs-site", agent: "Antigravity", ask: "Delete old/guide.md?", doing: "Checking 214 links", said: "Fixed 6 links; all pages build.", age: "1m" },
  ];
  // Where each chat is, and the way they go round.
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
  setInterval(() => {
    // One chat moves on each beat: a question answered, a task finished,
    // a new one started, and now and then an agent asks something.
    turn++;
    const i = turn % chats.length;
    if (turn % 5 === 0) state[i] = "needs";
    else state[i] = next[state[i]];
    if (!state.includes("needs")) state[(i + 1) % chats.length] = "needs";
    render();
  }, 2600);
})();

// ---- Typing an @ mention ------------------------------------------------
(() => {
  const text = $(".composer .text");
  const suggest = $(".suggest");
  const items = $$(".sugg", suggest);
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const type = async (chars, speed = 55) => {
    for (const char of chars) {
      text.append(char);
      await wait(speed + Math.random() * 40);
    }
  };
  const highlight = (index) => items.forEach((item, i) => item.classList.toggle("hi", i === index));
  if (reduceMotion) {
    text.innerHTML = 'Why does <span class="mention">@src/routes/bookings.rs</span> return 409 here? <span class="mention">@screenshot</span>';
    return;
  }
  const run = async () => {
    for (;;) {
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
      // The last characters typed become the mention.
      text.lastChild?.remove();
      [...text.childNodes].slice(-3).forEach((node) => node.remove());
      const mention = document.createElement("span");
      mention.className = "mention";
      mention.textContent = "@src/routes/bookings.rs";
      text.append(mention);
      await type(" return 409 here? ");
      await type("@", 120);
      highlight(3);
      suggest.classList.add("open");
      await wait(900);
      suggest.classList.remove("open");
      text.lastChild?.remove();
      const shot = document.createElement("span");
      shot.className = "mention";
      shot.textContent = "@screenshot";
      text.append(shot);
      await wait(2600);
    }
  };
  // Starts when it is on screen.
  new IntersectionObserver(([entry], observer) => {
    if (entry.isIntersecting) {
      observer.disconnect();
      run();
    }
  }).observe($(".composer-demo"));
})();

// ---- The scroll tour ----------------------------------------------------
(() => {
  const layers = Object.fromEntries($$(".tour-window .layer").map((layer) => [layer.dataset.layer, layer]));
  const spotlight = $(".spotlight");
  const steps = $$(".step");
  const show = (step) => {
    steps.forEach((other) => other.classList.toggle("on", other === step));
    const name = step.dataset.layer;
    for (const [key, layer] of Object.entries(layers)) {
      const on = key === name;
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
  let current = null;
  let queued = false;
  const update = () => {
    queued = false;
    const line = innerHeight * 0.55;
    const step = steps.filter((each) => each.getBoundingClientRect().top < line).pop() || steps[0];
    if (step !== current) {
      current = step;
      show(step);
    }
  };
  addEventListener(
    "scroll",
    () => {
      if (!queued) {
        queued = true;
        requestAnimationFrame(update);
      }
    },
    { passive: true }
  );
  update();
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

// ---- Cards: a spotlight and a slight tilt under the pointer -------------
if (finePointer && !reduceMotion) {
  for (const card of $$(".card")) {
    card.addEventListener("pointermove", (event) => {
      const box = card.getBoundingClientRect();
      const x = (event.clientX - box.left) / box.width;
      const y = (event.clientY - box.top) / box.height;
      card.style.setProperty("--mx", `${x * 100}%`);
      card.style.setProperty("--my", `${y * 100}%`);
      card.style.setProperty("--ty", `${(x - 0.5) * 8}deg`);
      card.style.setProperty("--tx", `${(0.5 - y) * 8}deg`);
    });
    card.addEventListener("pointerleave", () => {
      card.style.setProperty("--tx", "0deg");
      card.style.setProperty("--ty", "0deg");
    });
  }
}

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
})();
