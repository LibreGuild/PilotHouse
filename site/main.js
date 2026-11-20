// What moves on the page. Without this file everything is still there, drawn
// in place: it adds the night sky and the sea the agents float on, the
// scene that stays while the window comes up out of that sea, the window
// playing a task by itself, the one window the four moves change, the
// Bridge's cards going from column to column, the colour the whole page
// takes, and the sun coming up at the end.
//
// Each piece runs only while it is on screen, and none of them runs for a
// reader who asked their system for less motion. A piece that changes how
// something is laid out says so with a class on that thing (`pin`,
// `afloat`, `demo-on`, `pinned`, `staged`, `live`), so a style never
// counts on a script that did not run.

const root = document.documentElement;
const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const $ = (selector, scope = document) => scope.querySelector(selector);
const $$ = (selector, scope = document) => [...scope.querySelectorAll(selector)];
const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
/** 0 to 1, slow at both ends. */
const ease = (t) => t * t * (3 - 2 * t);

/** Calls `on(visible)` as `el` comes onto the screen and leaves it. */
function watch(el, on, margin = "0px") {
  if (!el) return;
  if (!("IntersectionObserver" in window)) {
    on(true);
    return;
  }
  new IntersectionObserver(
    (entries) => {
      for (const entry of entries) on(entry.isIntersecting);
    },
    { rootMargin: margin },
  ).observe(el);
}

// What follows the scroll, done once a frame however often the event fires.
const followers = [];
let queued = false;
const follow = () => {
  queued = false;
  for (const each of followers) each();
};
const queue = () => {
  if (queued) return;
  queued = true;
  requestAnimationFrame(follow);
};
addEventListener("scroll", queue, { passive: true });
addEventListener("resize", queue);

/** Runs `draw(seconds)` every frame while `box` is on screen. */
function animate(box, draw) {
  let time = 0;
  let last = 0;
  let frame = 0;
  let running = false;
  const loop = (now) => {
    if (!running) return;
    time += Math.min(50, now - last) * 0.001;
    last = now;
    draw(time);
    frame = requestAnimationFrame(loop);
  };
  watch(
    box,
    (visible) => {
      running = visible;
      cancelAnimationFrame(frame);
      if (visible) {
        last = performance.now();
        frame = requestAnimationFrame(loop);
      }
    },
    "120px",
  );
}

/** Sizes `canvas` to its box, sharp on a dense screen; returns the box's size. */
function fit(canvas, context) {
  const ratio = Math.min(devicePixelRatio || 1, 2);
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  return [width, height];
}

// The page's accent, as the sky and the sea draw it.
const accent = { first: "#4fd6c7", second: "#a5f3c4" };
const readAccent = () => {
  const style = getComputedStyle(root);
  const first = style.getPropertyValue("--accent").trim();
  const second = style.getPropertyValue("--accent-2").trim();
  if (/^#[0-9a-f]{6}$/i.test(first) && /^#[0-9a-f]{6}$/i.test(second)) Object.assign(accent, { first, second });
};
readAccent();

// ---- Headings, a word at a time -----------------------------------------
for (const heading of $$("[data-reveal]")) {
  const text = heading.textContent.trim();
  heading.setAttribute("aria-label", text);
  heading.textContent = "";
  text.split(/\s+/).forEach((word, index) => {
    const span = document.createElement("span");
    span.className = "w";
    span.setAttribute("aria-hidden", "true");
    span.style.setProperty("--i", index);
    span.textContent = word;
    heading.append(span, " ");
  });
  watch(heading, (visible) => visible && heading.classList.add("in"), "0px 0px -12% 0px");
}

// ---- The sky: stars that twinkle, and now and then one that falls --------
{
  const canvas = $("#sky");
  if (canvas) {
    const context = canvas.getContext("2d");
    let width = 0;
    let height = 0;
    let stars = [];
    let falling = null;
    let nextFall = 2.5;

    const size = () => {
      [width, height] = fit(canvas, context);
      // More of them high up, thinning toward the water.
      stars = Array.from({ length: Math.min(300, Math.round((width * height) / 4800)) }, () => ({
        x: Math.random() * width,
        y: Math.random() ** 1.6 * height * 0.88,
        r: Math.random() < 0.1 ? 1.5 : 0.4 + Math.random() * 0.7,
        phase: Math.random() * 6.283,
        speed: 0.4 + Math.random() * 1.7,
        tint: Math.random() < 0.28 ? "#dfe6ea" : "#ffffff",
      }));
    };

    const draw = (time) => {
      context.clearRect(0, 0, width, height);
      for (const star of stars) {
        const lit = reduce ? 0.8 : 0.55 + 0.45 * Math.sin(time * star.speed + star.phase);
        context.globalAlpha = 0.06 + lit * (1 - star.y / height) * 0.9;
        context.fillStyle = star.tint;
        context.beginPath();
        context.arc(star.x, star.y, star.r, 0, 6.283);
        context.fill();
      }
      if (falling) {
        const t = (time - falling.start) / 0.9;
        if (t >= 1) {
          falling = null;
        } else {
          const x = falling.x + falling.dx * t;
          const y = falling.y + falling.dy * t;
          const tail = context.createLinearGradient(x, y, x - falling.dx * 0.24, y - falling.dy * 0.24);
          tail.addColorStop(0, "rgba(255, 255, 255, 0.95)");
          tail.addColorStop(1, "rgba(255, 255, 255, 0)");
          context.globalAlpha = Math.sin(Math.PI * t);
          context.strokeStyle = tail;
          context.lineWidth = 1.6;
          context.beginPath();
          context.moveTo(x, y);
          context.lineTo(x - falling.dx * 0.24, y - falling.dy * 0.24);
          context.stroke();
        }
      } else if (time > nextFall) {
        falling = {
          start: time,
          x: width * (0.15 + Math.random() * 0.7),
          y: height * (0.04 + Math.random() * 0.22),
          dx: (Math.random() < 0.5 ? -1 : 1) * width * 0.26,
          dy: height * 0.2,
        };
        nextFall = time + 5 + Math.random() * 6;
      }
      context.globalAlpha = 1;
    };

    size();
    draw(0);
    addEventListener("resize", () => {
      size();
      draw(0);
    });
    if (!reduce) animate(canvas, draw);
  }
}

// ---- A sea: four lines of swell over water nothing shows through ----------
/** Draws a sea on `canvas` and floats the `.boat`s beside it on its
 * waves. `look` is its colours: by night under the hero, at sunrise at the
 * end of the page. */
function sea(canvas, look) {
  const box = canvas.parentElement;
  const context = canvas.getContext("2d");
  const boats = $$(".boat", box).map((el) => ({ el, at: Number(el.dataset.x), colour: el.style.getPropertyValue("--c").trim(), half: 0 }));
  // The nearest wave last: lower, slower and fainter.
  const waves = [
    { y: look.line, a: 8, k: 0.011, s: 0.7, o: 0.95 },
    { y: look.line + 0.17, a: 12, k: 0.008, s: 0.5, o: 0.5 },
    { y: look.line + 0.36, a: 16, k: 0.006, s: 0.38, o: 0.3 },
    { y: look.line + 0.55, a: 20, k: 0.0045, s: 0.28, o: 0.16 },
  ];
  // Glints on the water, thickest under the light in the middle.
  const glints = Array.from({ length: look.glints }, () => ({
    x: (Math.random() + Math.random() + Math.random() - 1.5) / 1.5,
    y: Math.random(),
    w: 6 + Math.random() * 22,
    phase: Math.random() * 6.283,
    speed: 0.8 + Math.random() * 1.8,
  }));
  let width = 0;
  let height = 0;
  let time = 0;

  const heightAt = (wave, x) =>
    height * wave.y +
    wave.a * Math.sin(x * wave.k + time * wave.s) +
    wave.a * 0.45 * Math.sin(x * wave.k * 2.3 - time * wave.s * 1.4 + 1.3);
  const trace = (wave) => {
    context.beginPath();
    context.moveTo(0, heightAt(wave, 0));
    for (let x = 8; x <= width + 8; x += 8) context.lineTo(x, heightAt(wave, x));
  };

  const size = () => {
    [width, height] = fit(canvas, context);
    // Half of each agent's width, so none drifts off a narrow screen.
    for (const boat of boats) boat.half = boat.el.offsetWidth / 2 + 8;
  };

  const draw = (now) => {
    time = now;
    const [first, middle, last] = look.lines();
    context.clearRect(0, 0, width, height);
    waves.forEach((wave, index) => {
      trace(wave);
      context.lineTo(width, height);
      context.lineTo(0, height);
      context.closePath();
      if (index === 0) {
        const water = context.createLinearGradient(0, height * wave.y - wave.a, 0, height);
        look.water.forEach((colour, stop) => water.addColorStop(stop / (look.water.length - 1), colour));
        context.globalAlpha = 1;
        context.fillStyle = water;
        context.fill();
      }
      const wash = context.createLinearGradient(0, height * wave.y, 0, height);
      wash.addColorStop(0, first);
      wash.addColorStop(1, `${first}00`);
      context.globalAlpha = wave.o * look.wash;
      context.fillStyle = wash;
      context.fill();
      trace(wave);
      const line = context.createLinearGradient(0, 0, width, 0);
      line.addColorStop(0, first);
      line.addColorStop(0.5, middle);
      line.addColorStop(1, last);
      context.globalAlpha = wave.o;
      context.strokeStyle = line;
      context.lineWidth = 1.5;
      context.stroke();
    });

    const top = waves[0];
    const depth = height - height * top.y;
    context.fillStyle = look.glint();
    for (const glint of glints) {
      const lit = Math.max(0, Math.sin(time * glint.speed + glint.phase)) ** 3;
      if (lit < 0.03) continue;
      const x = width * (0.5 + glint.x * look.spread);
      context.globalAlpha = lit * look.shine * (1 - glint.y * 0.7);
      context.fillRect(x - glint.w / 2, heightAt(top, x) + 8 + glint.y * depth * 0.7, glint.w, 1.5);
    }

    // Each agent rides the second wave (in front of the first, so it sails
    // on the water and not over what stands behind the sea), leaning with
    // its slope, its lamp on the water under it.
    const swell = waves[1];
    boats.forEach((boat, index) => {
      if (!boat.half || boat.half * 2 > width) return;
      const x = clamp(width * boat.at + Math.sin(time * 0.13 + index * 1.7) * 14, boat.half, width - boat.half);
      const y = heightAt(swell, x);
      const slope = (heightAt(swell, x + 14) - heightAt(swell, x - 14)) / 28;
      const glow = context.createRadialGradient(x, y + 12, 0, x, y + 12, 48);
      glow.addColorStop(0, `${boat.colour}55`);
      glow.addColorStop(1, `${boat.colour}00`);
      context.globalAlpha = 1;
      context.fillStyle = glow;
      context.fillRect(x - 50, y, 100, 62);
      boat.el.style.transform = `translate(${x}px, ${y}px) translate(-50%, calc(-100% + 5px)) rotate(${Math.atan(slope) * 46}deg)`;
    });
    context.globalAlpha = 1;
  };

  size();
  box.classList.add("afloat");
  size();
  draw(0);
  addEventListener("resize", () => {
    size();
    draw(time);
  });
  document.addEventListener("accent", () => reduce && draw(time));
  if (!reduce) animate(box, draw);
}

if ($("#sea")) {
  sea($("#sea"), {
    line: 0.34,
    // Black water: it ends in the colour of the section under it.
    water: ["#16181b", "#0e0f11", "#08090a"],
    wash: 0.14,
    lines: () => [accent.first, accent.second, accent.first],
    glint: () => accent.second,
    shine: 0.6,
    spread: 0.36,
    glints: 36,
  });
}
if ($("#sea-day")) {
  sea($("#sea-day"), {
    line: 0.52,
    // It ends in the colour of the daylight section under it.
    water: ["#ec9c84", "#f5cba9", "#f6f1e7"],
    wash: 0.2,
    lines: () => ["#fff1dc", "#ffffff", "#ffe3c4"],
    glint: () => "#fffdf2",
    shine: 0.95,
    spread: 0.2,
    glints: 48,
  });
}

// ---- The hero's scene stays while the window comes up ----------------------
{
  const hero = $(".hero");
  const scene = $(".scene");
  const text = $(".hero-text");
  const stage = $(".hero-stage");
  const win = $("#demo");
  const nav = $(".nav");
  if (hero && scene && text && stage && win && nav && !reduce) {
    // Only where there is room for the words, the window and the sea at once.
    const roomy = matchMedia("(min-width: 1001px) and (min-height: 560px)");
    let pinned = false;

    const scroll = () => {
      if (!pinned) return;
      const span = hero.offsetHeight - scene.clientHeight;
      const p = ease(clamp((scrollY - hero.offsetTop) / span, 0, 1));
      scene.style.setProperty("--p", p.toFixed(4));
      // Out of the tab order once they can no longer be seen.
      text.classList.toggle("gone", p > 0.45);
    };

    const layout = () => {
      pinned = roomy.matches;
      hero.classList.toggle("pin", pinned);
      if (!pinned) {
        text.classList.remove("gone");
        return;
      }
      const high = scene.clientHeight;
      const top = nav.offsetHeight;
      // Where the window ends up: under the header and the task's phases,
      // as large as fits with a strip of sea under it (larger than drawn
      // on a big screen, where it would otherwise look lost).
      const room = high - top - 112;
      const widest = (scene.clientWidth - 96) / stage.offsetWidth;
      const end = clamp(room / win.offsetHeight, 0.72, Math.min(1.3, widest));
      const y1 = top + 60 + Math.max(0, room - win.offsetHeight * end) * 0.35;
      // Where it starts: just under the words.
      const y0 = Math.max(text.offsetTop + text.offsetHeight + clamp(high * 0.045, 20, 44), y1);
      scene.style.setProperty("--y0", `${y0.toFixed(1)}px`);
      scene.style.setProperty("--y1", `${y1.toFixed(1)}px`);
      scene.style.setProperty("--s1", end.toFixed(4));
      scene.style.setProperty("--s0", (end * 0.88).toFixed(4));
      scroll();
    };

    layout();
    followers.push(scroll);
    addEventListener("resize", layout);
    addEventListener("load", layout);
    roomy.addEventListener("change", layout);
    document.fonts?.ready.then(layout);

    // "Watch a task go by" brings the window to the middle.
    $("#watch")?.addEventListener("click", (event) => {
      if (!pinned) return;
      event.preventDefault();
      scrollTo({ top: hero.offsetTop + hero.offsetHeight - scene.clientHeight + 1, behavior: "smooth" });
    });

    // The window leans a little toward the pointer.
    if (matchMedia("(hover: hover)").matches) {
      scene.addEventListener("pointermove", (event) => {
        const box = scene.getBoundingClientRect();
        win.style.setProperty("--ry", `${(((event.clientX - box.left) / box.width - 0.5) * 6).toFixed(2)}deg`);
        win.style.setProperty("--rx", `${((0.5 - (event.clientY - box.top) / box.height) * 4).toFixed(2)}deg`);
      });
      scene.addEventListener("pointerleave", () => {
        win.style.removeProperty("--ry");
        win.style.removeProperty("--rx");
      });
    }
  }
}

// ---- The hero's window plays a task ---------------------------------------
{
  const win = $("#demo");
  if (win && !reduce) {
    const part = (name) => $(`[data-d="${name}"]`, win);
    const hook = (name) => $(`[data-demo="${name}"]`, win);
    const typed = hook("typed");
    const pointer = hook("pointer");
    const tabDot = hook("tabdot");
    const ask = hook("ask");
    const [chatIdle, paneIdle] = $$("[data-idle]", win);
    const phases = $$(".phases li");
    const REQUEST = "Show the tide chart when there is no network.";
    // Each start gets a number; a run stops when it sees a newer one (the
    // window left the screen, or came back).
    let run = 0;

    /** Lights the phase the task is in, and ticks the ones before it. */
    const phase = (name) => {
      const at = phases.findIndex((item) => item.dataset.phase === name);
      phases.forEach((item, index) => {
        item.classList.toggle("on", index === at);
        item.classList.toggle("done", name === "all" || (at >= 0 && index < at));
      });
    };

    /** Moves the pointer to a point of `el`, in the window's own
     * coordinates (it is drawn leaning, which the layout does not know). */
    const pointAt = (el, dx = 0.5, dy = 0.62) => {
      // On a narrow screen the result pane is not shown: nothing to point at.
      if (!el.offsetParent) return;
      let x = 0;
      let y = 0;
      for (let node = el; node && node !== win; node = node.offsetParent) {
        x += node.offsetLeft;
        y += node.offsetTop;
      }
      pointer.style.transform = `translate(${x + el.offsetWidth * dx}px, ${y + el.offsetHeight * dy}px)`;
    };

    const press = async (el) => {
      el.classList.remove("pressed");
      void el.offsetWidth;
      el.classList.add("pressed");
      await sleep(320);
    };

    const reset = () => {
      for (const el of $$("[data-d]", win)) el.classList.remove("show");
      for (const tick of ["t1", "t2"]) hook(tick).classList.remove("on");
      ask.classList.remove("gone");
      chatIdle.classList.remove("off");
      paneIdle.classList.remove("off");
      // A phone shows one pane of the window at a time: the chat first.
      win.dataset.view = "chat";
      typed.textContent = "";
      tabDot.className = "dot";
      phase("none");
      pointer.style.transform = `translate(${win.offsetWidth * 0.5}px, ${win.offsetHeight * 0.86}px)`;
    };

    const play = async (mine) => {
      const stale = () => mine !== run;
      const show = (name) => part(name).classList.add("show");
      while (!stale()) {
        reset();
        await sleep(700);
        if (stale()) return;
        // The request is typed and sent.
        phase("ask");
        for (let length = 1; length <= REQUEST.length; length++) {
          typed.textContent = REQUEST.slice(0, length);
          await sleep(22);
          if (stale()) return;
        }
        pointAt(hook("send"));
        await sleep(800);
        if (stale()) return;
        await press(hook("send"));
        typed.textContent = "";
        chatIdle.classList.add("off");
        show("msg");
        tabDot.className = "dot blue";
        phase("work");
        // The agent works: its steps on the left, what it changed on the right.
        for (const [name, wait] of [["s1", 650], ["s2", 750], ["f1", 150], ["diff", 450], ["s3", 750], ["f2", 150]]) {
          await sleep(wait);
          if (stale()) return;
          if (name === "f1") paneIdle.classList.add("off");
          show(name);
        }
        // It asks; the answer is a click.
        await sleep(750);
        if (stale()) return;
        show("ask");
        show("count");
        tabDot.className = "dot amber";
        phase("allow");
        await sleep(850);
        if (stale()) return;
        pointAt(hook("allow"));
        await sleep(950);
        if (stale()) return;
        await press(hook("allow"));
        ask.classList.add("gone");
        part("count").classList.remove("show");
        tabDot.className = "dot blue";
        await sleep(850);
        if (stale()) return;
        show("tests");
        tabDot.className = "dot green";
        phase("review");
        // The review: on a phone the Git pane takes the chat's place; then
        // each file is ticked as read, and the change committed.
        await sleep(750);
        if (stale()) return;
        win.dataset.view = "result";
        for (const tick of ["t1", "t2"]) {
          await sleep(450);
          if (stale()) return;
          pointAt(hook(tick), 0.5, 0.5);
          await sleep(750);
          if (stale()) return;
          hook(tick).classList.add("on");
        }
        await sleep(400);
        if (stale()) return;
        show("commit");
        phase("ship");
        await sleep(400);
        if (stale()) return;
        pointAt(hook("commit"));
        await sleep(950);
        if (stale()) return;
        await press(hook("commit"));
        show("toast");
        phase("all");
        await sleep(3600);
      }
    };

    win.classList.add("demo-on");
    reset();
    watch(win, (visible) => {
      run += 1;
      if (visible) void play(run);
    });
  }
}

// ---- A task in four moves: one window, changed by the scroll ----------------
{
  const section = $(".moves");
  const stage = section && $(".stage-frames", section);
  if (section && stage) {
    const items = $$(".move", section);
    const dots = $$(".stage-dots li", section);
    const frames = items.map((item) => {
      const frame = document.createElement("div");
      frame.className = "frame";
      frame.append($(".pane", item).cloneNode(true));
      stage.append(frame);
      return frame;
    });
    const show = (index) => {
      items.forEach((item, i) => item.classList.toggle("on", i === index));
      dots.forEach((dot, i) => dot.classList.toggle("on", i === index));
      frames.forEach((frame, i) => {
        frame.classList.toggle("on", i === index);
        frame.classList.toggle("gone", i < index);
      });
    };
    // Pinned only where there is room for the two side by side.
    const wide = matchMedia("(min-width: 1001px)");
    const mode = () => section.classList.toggle("pinned", wide.matches && !reduce);
    mode();
    wide.addEventListener("change", mode);
    // The move nearest the middle of the window is the one shown.
    const pick = () => {
      if (!section.classList.contains("pinned")) return;
      const middle = innerHeight / 2;
      let best = 0;
      let nearest = Infinity;
      items.forEach((item, i) => {
        const box = item.getBoundingClientRect();
        const away = Math.abs((box.top + box.bottom) / 2 - middle);
        if (away < nearest) {
          nearest = away;
          best = i;
        }
      });
      show(best);
    };
    followers.push(pick);
    show(0);
  }
}

// ---- The Bridge: cards go from column to column -----------------------------
{
  const board = $("#board");
  if (board && "animate" in Element.prototype) {
    const columns = Object.fromEntries($$(".col", board).map((col) => [col.dataset.col, col]));
    const cards = Object.fromEntries($$(".card", board).map((card) => [card.dataset.id, card]));
    const start = Object.entries(cards).map(([id, card]) => [id, card.dataset.state]);
    const count = () => {
      for (const col of Object.values(columns)) $("small", col).textContent = $$(".card", col).length;
    };

    /** Puts a card in the column of its new state; it, and the cards that
     * make room, slide from where they were. */
    const move = (id, state) => {
      const all = Object.values(cards);
      const before = new Map(all.map((card) => [card, card.getBoundingClientRect()]));
      cards[id].dataset.state = state;
      columns[state].append(cards[id]);
      count();
      if (reduce) return;
      for (const card of all) {
        const was = before.get(card);
        const now = card.getBoundingClientRect();
        const dx = was.left - now.left;
        const dy = was.top - now.top;
        if (!dx && !dy) continue;
        const moved = card === cards[id];
        card.classList.toggle("moving", moved);
        card.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], {
          duration: moved ? 750 : 450,
          easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
        }).onfinish = () => card.classList.remove("moving");
      }
    };

    // What happens on an ordinary afternoon.
    const script = [
      ["a", "working"],
      ["d", "done"],
      ["b", "ready"],
      ["c", "needs"],
      ["a", "ready"],
      ["c", "working"],
      ["b", "done"],
    ];
    let run = 0;
    let visible = false;
    /** Plays the afternoon over and over, after `pause` ms, until a newer
     * run starts. A card a reader already moved is left where it is. */
    const play = async (mine, pause = 0) => {
      await sleep(pause);
      while (mine === run) {
        for (const [id, state] of script) {
          await sleep(1900);
          if (mine !== run) return;
          if (cards[id].dataset.state !== state) move(id, state);
        }
        await sleep(2800);
        if (mine !== run) return;
        for (const [id, state] of start) if (cards[id].dataset.state !== state) move(id, state);
      }
    };
    count();
    if (!reduce) {
      watch(board, (now) => {
        visible = now;
        run += 1;
        if (visible) void play(run);
      });
    }

    // A reader can answer too: Allow or Deny sends the agent back to work,
    // Review changes finishes the task. The board waits a moment, then
    // carries on by itself.
    board.classList.add("live");
    board.addEventListener("click", (event) => {
      const card = event.target.closest(".wbtn")?.closest(".card");
      const next = { needs: "working", ready: "done" }[card?.dataset.state];
      if (!next) return;
      run += 1;
      move(card.dataset.id, next);
      if (visible && !reduce) void play(run, 2600);
    });
  }
}

// ---- The tiles: drawings that move on screen, a light under the pointer ------
{
  const grid = $(".bento");
  if (grid) {
    const tiles = $$(".tile", grid);
    grid.classList.add("staged");
    for (const tile of tiles) watch(tile, (visible) => tile.classList.toggle("play", visible));
    if (matchMedia("(hover: hover)").matches) {
      grid.addEventListener("pointermove", (event) => {
        grid.classList.add("lit");
        for (const tile of tiles) {
          const box = tile.getBoundingClientRect();
          tile.style.setProperty("--mx", `${(event.clientX - box.left).toFixed(0)}px`);
          tile.style.setProperty("--my", `${(event.clientY - box.top).toFixed(0)}px`);
        }
      });
      grid.addEventListener("pointerleave", () => grid.classList.remove("lit"));
    }
  }
}

// ---- Make it yours: the whole page takes the colour -------------------------
{
  // The accent, the colour its gradients run to, and its shade for daylight.
  const ACCENTS = {
    teal: ["#4fd6c7", "#a5f3c4", "#0d8377"],
    green: ["#63d98f", "#d6f58b", "#1f8a50"],
    blue: ["#6ea8ff", "#8fe3ff", "#2a66d4"],
    violet: ["#a78bfa", "#f0abfc", "#5a3fd0"],
    amber: ["#f5b83d", "#ff9e7a", "#a96a05"],
    pink: ["#f58bc8", "#ffb199", "#c0307c"],
  };
  const buttons = $$(".swatches button");
  for (const button of buttons) {
    button.addEventListener("click", () => {
      const [first, second, day] = ACCENTS[button.dataset.accent];
      root.style.setProperty("--accent", first);
      root.style.setProperty("--accent-2", second);
      root.style.setProperty("--accent-day", day);
      root.style.setProperty("--accent-soft", `${first}29`);
      for (const other of buttons) other.setAttribute("aria-pressed", String(other === button));
      readAccent();
      document.dispatchEvent(new Event("accent"));
    });
  }
}

// ---- The header: how far down, which section, and daylight ------------------
{
  const nav = $(".nav");
  const read = $(".read");
  const links = $$(".nav nav a");
  const sections = links.map((link) => $(link.getAttribute("href")));
  const end = $("#download");
  const daybreak = $(".daybreak");
  if (nav) {
    followers.push(() => {
      const most = root.scrollHeight - innerHeight;
      read?.style.setProperty("--read", most > 0 ? (scrollY / most).toFixed(4) : "0");
      // The section that has reached the upper part of the window is the
      // current one; past the last of them, none is.
      const line = innerHeight * 0.4;
      let current = -1;
      sections.forEach((section, index) => {
        if (section && section.getBoundingClientRect().top <= line) current = index;
      });
      if (end && end.getBoundingClientRect().top <= line) current = -1;
      links.forEach((link, index) => {
        if (index === current) link.setAttribute("aria-current", "location");
        else link.removeAttribute("aria-current");
      });
      if (!daybreak) return;
      const box = daybreak.getBoundingClientRect();
      // The header turns light once the sunrise is under it.
      nav.classList.toggle("day", box.top + box.height * 0.2 < nav.offsetHeight);
      // And the sun comes up as its sea is scrolled into view.
      if (!reduce) daybreak.style.setProperty("--rise", ease(clamp((innerHeight - box.top) / (box.height * 0.9), 0, 1)).toFixed(4));
    });
  }
}

// ---- The download for this computer ----------------------------------------
{
  const platform = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
  const phone = /android|iphone|ipad/i.test(navigator.userAgent);
  const os = phone
    ? null
    : platform.includes("mac")
      ? "mac"
      : platform.includes("win")
        ? "windows"
        : platform.includes("linux")
          ? "linux"
          : null;
  const names = { mac: "macOS", windows: "Windows", linux: "Linux" };
  const mine = os && $(`.dl[data-os="${os}"]`);
  if (mine) {
    mine.classList.add("mine");
    const main = $("#download-main");
    if (main) {
      main.href = mine.href;
      $("span", main).textContent = `Download for ${names[os]}`;
    }
  }
}

follow();
