// What moves on the page. Without this file everything is still there, drawn
// in place: it adds the sea the agents float on, the window that plays a
// task by itself, the one window the four moves change, the Bridge's cards
// going from column to column, and the colour the whole page takes.
//
// Each piece runs only while it is on screen, and none of them runs for a
// reader who asked their system for less motion.

document.documentElement.classList.add("js");

const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

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

// ---- The sea, and the agents afloat on it -------------------------------
{
  const canvas = $("#sea");
  const box = canvas?.parentElement;
  if (canvas && box) {
    const context = canvas.getContext("2d");
    const boats = $$(".boat", box);
    // Four lines of swell, the nearest last: lower, slower and fainter.
    const waves = [
      { y: 0.36, a: 9, k: 0.011, s: 0.7, o: 0.9 },
      { y: 0.52, a: 13, k: 0.008, s: 0.5, o: 0.55 },
      { y: 0.7, a: 17, k: 0.006, s: 0.38, o: 0.32 },
      { y: 0.88, a: 21, k: 0.0045, s: 0.28, o: 0.18 },
    ];
    let width = 0;
    let height = 0;
    let time = 0;
    let last = 0;
    let running = false;
    let frame = 0;
    let colors = ["#a78bfa", "#f0abfc"];
    // Half of each agent's width, so none of them drifts off a narrow screen.
    let halves = [];

    const heightAt = (wave, x) =>
      height * wave.y +
      wave.a * Math.sin(x * wave.k + time * wave.s) +
      wave.a * 0.45 * Math.sin(x * wave.k * 2.3 - time * wave.s * 1.4 + 1.3);

    const readColors = () => {
      const style = getComputedStyle(document.documentElement);
      const first = style.getPropertyValue("--accent").trim();
      const second = style.getPropertyValue("--accent-2").trim();
      if (/^#[0-9a-f]{6}$/i.test(first) && /^#[0-9a-f]{6}$/i.test(second)) colors = [first, second];
    };

    const size = () => {
      const ratio = Math.min(devicePixelRatio || 1, 2);
      width = box.clientWidth;
      height = box.clientHeight;
      canvas.width = width * ratio;
      canvas.height = height * ratio;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      halves = boats.map((boat) => boat.offsetWidth / 2 + 8);
    };

    const draw = () => {
      context.clearRect(0, 0, width, height);
      for (const wave of waves) {
        context.beginPath();
        context.moveTo(0, heightAt(wave, 0));
        for (let x = 8; x <= width + 8; x += 8) context.lineTo(x, heightAt(wave, x));
        const line = context.createLinearGradient(0, 0, width, 0);
        line.addColorStop(0, colors[0]);
        line.addColorStop(0.5, "#6ea8ff");
        line.addColorStop(1, colors[1]);
        context.globalAlpha = wave.o;
        context.strokeStyle = line;
        context.lineWidth = 1.5;
        context.stroke();
        // The water under the line, fading down.
        context.lineTo(width, height);
        context.lineTo(0, height);
        context.closePath();
        const water = context.createLinearGradient(0, height * wave.y, 0, height);
        water.addColorStop(0, colors[0]);
        water.addColorStop(1, `${colors[0]}00`);
        context.globalAlpha = wave.o * 0.16;
        context.fillStyle = water;
        context.fill();
      }
      context.globalAlpha = 1;
      // Each agent rides the first line, leaning with its slope.
      const top = waves[0];
      boats.forEach((boat, index) => {
        const half = halves[index];
        const x = Math.min(width - half, Math.max(half, width * Number(boat.dataset.x)));
        const y = heightAt(top, x);
        const slope = (heightAt(top, x + 14) - heightAt(top, x - 14)) / 28;
        boat.style.transform = `translate(${x}px, ${y}px) translate(-50%, -108%) rotate(${Math.atan(slope) * 46}deg)`;
      });
    };

    const loop = (now) => {
      if (!running) return;
      time += Math.min(50, now - last) * 0.001;
      last = now;
      draw();
      frame = requestAnimationFrame(loop);
    };

    readColors();
    size();
    draw();
    addEventListener("resize", () => {
      size();
      draw();
    });
    document.addEventListener("accent", () => {
      readColors();
      if (!running) draw();
    });
    if (!reduce) {
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
  }
}

// ---- The hero's window leans toward the pointer --------------------------
{
  const stage = $(".hero-stage");
  const win = $("#demo");
  if (stage && win && !reduce && matchMedia("(hover: hover)").matches) {
    stage.addEventListener("pointermove", (event) => {
      const box = stage.getBoundingClientRect();
      const x = (event.clientX - box.left) / box.width - 0.5;
      const y = (event.clientY - box.top) / box.height - 0.5;
      win.style.setProperty("--ry", `${x * 7}deg`);
      win.style.setProperty("--rx", `${4 - y * 7}deg`);
    });
    stage.addEventListener("pointerleave", () => {
      win.style.removeProperty("--ry");
      win.style.removeProperty("--rx");
    });
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
    const REQUEST = "Show the tide chart when there is no network.";
    // Each start gets a number; a run stops when it sees a newer one (the
    // window left the screen, or came back).
    let run = 0;

    /** Moves the pointer to a point of `el`, in the window's own
     * coordinates (it is drawn leaning, which the layout does not know). */
    const pointAt = (el, dx = 0.5, dy = 0.62) => {
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
      typed.textContent = "";
      tabDot.className = "dot";
      pointer.style.transform = `translate(${win.offsetWidth * 0.5}px, ${win.offsetHeight * 0.92}px)`;
    };

    const play = async (mine) => {
      const stale = () => mine !== run;
      const show = (name) => part(name).classList.add("show");
      while (!stale()) {
        reset();
        await sleep(800);
        if (stale()) return;
        // The request is typed and sent.
        for (let length = 1; length <= REQUEST.length; length++) {
          typed.textContent = REQUEST.slice(0, length);
          await sleep(24);
          if (stale()) return;
        }
        pointAt(hook("send"));
        await sleep(850);
        if (stale()) return;
        await press(hook("send"));
        typed.textContent = "";
        show("msg");
        tabDot.className = "dot blue";
        // The agent works: its steps on the left, what it changed on the right.
        for (const [name, wait] of [["s1", 700], ["s2", 800], ["f1", 150], ["diff", 500], ["s3", 800], ["f2", 150]]) {
          await sleep(wait);
          if (stale()) return;
          show(name);
        }
        // It asks; the answer is a click.
        await sleep(800);
        if (stale()) return;
        show("ask");
        show("count");
        tabDot.className = "dot amber";
        await sleep(900);
        if (stale()) return;
        pointAt(hook("allow"));
        await sleep(950);
        if (stale()) return;
        await press(hook("allow"));
        ask.classList.add("gone");
        part("count").classList.remove("show");
        tabDot.className = "dot blue";
        await sleep(900);
        if (stale()) return;
        show("tests");
        tabDot.className = "dot green";
        // The review: each file ticked as read, then the commit.
        for (const tick of ["t1", "t2"]) {
          await sleep(500);
          if (stale()) return;
          pointAt(hook(tick), 0.5, 0.5);
          await sleep(750);
          if (stale()) return;
          hook(tick).classList.add("on");
        }
        await sleep(400);
        show("commit");
        await sleep(400);
        if (stale()) return;
        pointAt(hook("commit"));
        await sleep(950);
        if (stale()) return;
        await press(hook("commit"));
        show("toast");
        await sleep(3400);
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
    let waiting = false;
    const pick = () => {
      waiting = false;
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
    addEventListener(
      "scroll",
      () => {
        if (waiting) return;
        waiting = true;
        requestAnimationFrame(pick);
      },
      { passive: true },
    );
    show(0);
    pick();
  }
}

// ---- The Bridge: cards go from column to column -----------------------------
{
  const board = $("#board");
  if (board) {
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
    const play = async (mine) => {
      while (mine === run) {
        for (const [id, state] of script) {
          await sleep(1900);
          if (mine !== run) return;
          move(id, state);
        }
        await sleep(2800);
        if (mine !== run) return;
        for (const [id, state] of start) move(id, state);
      }
    };
    count();
    if (!reduce && "animate" in Element.prototype) {
      watch(board, (visible) => {
        run += 1;
        if (visible) void play(run);
      });
    }
  }
}

// ---- A tile's drawing moves while it is on screen ---------------------------
for (const tile of $$(".tile")) watch(tile, (visible) => tile.classList.toggle("play", visible));

// ---- Make it yours: the whole page takes the colour -------------------------
{
  // The accent, the colour its gradients run to, and its shade for daylight.
  const ACCENTS = {
    violet: ["#a78bfa", "#f0abfc", "#5a3fd0"],
    blue: ["#6ea8ff", "#8fe3ff", "#2a66d4"],
    teal: ["#4fd6c7", "#a5f3c4", "#0d8377"],
    green: ["#63d98f", "#d6f58b", "#1f8a50"],
    amber: ["#f5b83d", "#ff9e7a", "#a96a05"],
    pink: ["#f58bc8", "#ffb199", "#c0307c"],
  };
  const buttons = $$(".swatches button");
  for (const button of buttons) {
    button.addEventListener("click", () => {
      const [accent, second, day] = ACCENTS[button.dataset.accent];
      const style = document.documentElement.style;
      style.setProperty("--accent", accent);
      style.setProperty("--accent-2", second);
      style.setProperty("--accent-day", day);
      style.setProperty("--accent-soft", `${accent}29`);
      for (const other of buttons) other.setAttribute("aria-pressed", String(other === button));
      document.dispatchEvent(new Event("accent"));
    });
  }
}

// ---- The header takes daylight when the page under it does ------------------
{
  const nav = $(".nav");
  const day = $(".download");
  if (nav && day && "IntersectionObserver" in window) {
    // The header turns light as the daylight section nears the top: the end
    // of the sunrise above it is already too bright for the night header.
    new IntersectionObserver(
      (entries) => {
        for (const entry of entries) nav.classList.toggle("day", entry.isIntersecting);
      },
      { rootMargin: "-30px 0px -70% 0px" },
    ).observe(day);
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
