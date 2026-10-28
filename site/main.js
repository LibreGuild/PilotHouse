// The navigation bar gets a background once the page scrolls, sections fade
// in as they come into view, and the download for this computer is marked.

const nav = document.querySelector(".nav");
const onScroll = () => nav.classList.toggle("scrolled", window.scrollY > 8);
onScroll();
window.addEventListener("scroll", onScroll, { passive: true });

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
if (!reduceMotion && "IntersectionObserver" in window) {
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          entry.target.classList.add("shown");
          observer.unobserve(entry.target);
        }
      }
    },
    { rootMargin: "0px 0px -10% 0px" }
  );
  const targets = document.querySelectorAll(
    ".section h2, .section .body, .split .art, .checks, .stats, .compare, .card, .download-card, .pills, .source"
  );
  for (const target of targets) {
    target.classList.add("reveal");
    observer.observe(target);
  }
}

// Marks the build for this computer, and points the main button at it.
const platform = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
const os = platform.includes("mac")
  ? "mac"
  : platform.includes("win")
    ? "windows"
    : platform.includes("linux") && !/android/i.test(navigator.userAgent)
      ? "linux"
      : null;
if (os) {
  const card = document.querySelector(`.download-card[data-os="${os}"]`);
  const main = document.getElementById("download-main");
  if (card && main) {
    card.classList.add("mine");
    main.textContent = `Download for ${card.querySelector(".os").textContent}`;
    main.href = card.href;
  }
}
