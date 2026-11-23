// A small right-click menu. One open at a time; closes on Esc, a click elsewhere,
// scrolling, resizing or the window losing focus.

export type MenuItem = { label: string; run: () => void; danger?: boolean; disabled?: boolean } | "sep";

let open: HTMLElement | null = null;
let restoreFocus: HTMLElement | null = null;

export function closeMenu() {
  if (!open) return;
  open.remove();
  open = null;
  restoreFocus?.focus?.();
  restoreFocus = null;
}

export function menuOpen(): boolean {
  return !!open;
}

export function showMenu(x: number, y: number, items: MenuItem[], label = "Menu") {
  closeMenu();
  restoreFocus = document.activeElement as HTMLElement | null;
  const m = document.createElement("div");
  m.className = "ctx-menu";
  m.setAttribute("role", "menu");
  m.setAttribute("aria-label", label);
  items.forEach((it) => {
    if (it === "sep") {
      m.insertAdjacentHTML("beforeend", '<div class="ctx-sep" role="separator"></div>');
      return;
    }
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.className = `ctx-item${it.danger ? " danger" : ""}`;
    b.textContent = it.label;
    b.disabled = !!it.disabled;
    b.addEventListener("click", (ev) => {
      ev.stopPropagation();
      closeMenu();
      it.run();
    });
    m.appendChild(b);
  });
  document.body.appendChild(m);
  // Keep it inside the window.
  const r = m.getBoundingClientRect();
  m.style.left = `${Math.max(4, Math.min(x, innerWidth - r.width - 4))}px`;
  m.style.top = `${Math.max(4, Math.min(y, innerHeight - r.height - 4))}px`;
  open = m;
  m.querySelector<HTMLButtonElement>(".ctx-item:not(:disabled)")?.focus();
}

document.addEventListener("mousedown", (ev) => {
  if (open && !open.contains(ev.target as Node)) closeMenu();
});
document.addEventListener("keydown", (ev) => {
  if (!open) return;
  if (ev.key === "Escape") {
    ev.stopImmediatePropagation();
    closeMenu();
    return;
  }
  if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
    ev.preventDefault();
    const btns = [...open.querySelectorAll<HTMLButtonElement>(".ctx-item:not(:disabled)")];
    const i = btns.indexOf(document.activeElement as HTMLButtonElement);
    const next = ev.key === "ArrowDown" ? (i + 1) % btns.length : (i - 1 + btns.length) % btns.length;
    btns[next]?.focus();
  }
}, true);
for (const e of ["resize", "blur"]) addEventListener(e, closeMenu);
document.addEventListener("scroll", closeMenu, true);
