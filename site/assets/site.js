// Install tabs, copy buttons, active section in the sidebar and TUI-like keys.

// Tabs: pick the visitor's OS first.
const os = /Win/.test(navigator.platform) ? "windows" : /Mac/.test(navigator.platform) ? "macos" : "linux";
document.querySelectorAll("[role=tablist]").forEach((list) => {
  const tabs = [...list.querySelectorAll("[role=tab]")];
  const show = (tab) =>
    tabs.forEach((t) => {
      const on = t === tab;
      t.setAttribute("aria-selected", on);
      document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
    });
  tabs.forEach((t) => t.addEventListener("click", () => show(t)));
  show(tabs.find((t) => t.dataset.os === os) || tabs[0]);
});

// Copy buttons next to each command.
document.querySelectorAll(".cmd").forEach((cmd) => {
  const btn = document.createElement("button");
  btn.className = "btn copy";
  btn.type = "button";
  const label = document.documentElement.lang === "es" ? ["[copiar]", "[copiado]"] : ["[copy]", "[copied]"];
  btn.textContent = label[0];
  btn.addEventListener("click", async () => {
    await navigator.clipboard.writeText(cmd.querySelector("code").textContent.trim());
    btn.textContent = label[1];
    setTimeout(() => (btn.textContent = label[0]), 1500);
  });
  cmd.append(btn);
});

// Highlight the section in view.
const links = new Map([...document.querySelectorAll(".nav a")].map((a) => [a.getAttribute("href").slice(1), a]));
const observer = new IntersectionObserver(
  (entries) =>
    entries.forEach((e) => {
      if (!e.isIntersecting) return;
      links.forEach((a) => a.classList.remove("active"));
      links.get(e.target.id)?.classList.add("active");
    }),
  { rootMargin: "-20% 0px -70% 0px" }
);
document.querySelectorAll("main section[id]").forEach((s) => observer.observe(s));

// Keys: 1–9 jump to a section, l switches language, g opens GitHub.
document.addEventListener("keydown", (e) => {
  if (e.ctrlKey || e.metaKey || e.altKey || /input|textarea/i.test(e.target.tagName)) return;
  const nav = [...links.values()];
  if (/^[1-9]$/.test(e.key) && nav[e.key - 1]) nav[e.key - 1].click();
  else if (e.key === "l") document.querySelector("[data-switch-lang]")?.click();
  else if (e.key === "g") window.open("https://github.com/abelland-lgtm/tbard", "_blank", "noopener");
});
