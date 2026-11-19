// The page works without this file. It adds three small things: the button
// that switches between light and dark, the download for the reader's own
// system, and the drawings coming in as they are scrolled to.

document.documentElement.classList.add("js");

// ---- Light or dark -----------------------------------------------------
// The system's choice, until the reader makes their own (kept on this
// computer, nowhere else).
const lookButton = document.getElementById("look");
lookButton?.addEventListener("click", () => {
  const root = document.documentElement;
  const dark = root.dataset.look
    ? root.dataset.look === "dark"
    : matchMedia("(prefers-color-scheme: dark)").matches;
  const next = dark ? "light" : "dark";
  root.dataset.look = next;
  try {
    localStorage.setItem("look", next);
  } catch (error) {
    // Storage is blocked: the choice lasts for this visit.
  }
});

// ---- The download for this computer ------------------------------------
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
  const mine = os && document.querySelector(`.dl[data-os="${os}"]`);
  if (mine) {
    mine.classList.add("mine");
    const main = document.getElementById("download-main");
    if (main) {
      main.href = mine.href;
      main.querySelector("span").textContent = `Download for ${names[os]}`;
    }
  }
}

// ---- Drawings come in as they are reached ------------------------------
{
  const drawings = document.querySelectorAll(".wf");
  if ("IntersectionObserver" in window) {
    const seen = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add("in");
            seen.unobserve(entry.target);
          }
        }
      },
      { rootMargin: "0px 0px -8% 0px" },
    );
    for (const drawing of drawings) seen.observe(drawing);
  } else {
    for (const drawing of drawings) drawing.classList.add("in");
  }
}
