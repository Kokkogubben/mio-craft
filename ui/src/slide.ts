// Selection highlights that glide from the old item to the new one instead of jumping, and
// slide-ins for whole panels.
//
// The lists are re-rendered with innerHTML, which would throw a highlight inside them away, so each
// highlight is kept here and put back into its list on every render.

type Pill = { el: HTMLElement; container: HTMLElement | null; active: HTMLElement | null };

const pills = new Map<string, Pill>();
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
const resized = new ResizeObserver(() => resync());
const observed = new Set<HTMLElement>();

/** Move the `name` highlight inside `container` onto `active` (or hide it if there's none). */
export function slide(name: string, container: HTMLElement | null, active: HTMLElement | null) {
  let p = pills.get(name);
  if (!p) {
    const el = document.createElement("div");
    el.className = `slide-pill slide-${name}`;
    el.setAttribute("aria-hidden", "true");
    p = { el, container: null, active: null };
    pills.set(name, p);
  }
  if (p.container !== container) {
    if (p.container && observed.delete(p.container)) resized.unobserve(p.container);
    if (container && !observed.has(container)) {
      observed.add(container);
      resized.observe(container);
    }
  }
  p.container = container;
  p.active = active;
  place(p, false);
}

/** Put every highlight straight onto its item, e.g. after a view became visible or a resize. */
export function resync() {
  for (const p of pills.values()) place(p, true);
}

function place(p: Pill, instant: boolean) {
  const { el, container, active } = p;
  if (!container || !container.isConnected) {
    el.remove();
    delete el.dataset.at;
    return;
  }
  const moved = el.parentElement !== container;
  if (moved) container.prepend(el);
  // Hidden (another view is showing): nothing to measure, so place it fresh next time.
  if (!active || !active.isConnected || !active.offsetParent) {
    el.classList.toggle("on", false);
    delete el.dataset.at;
    return;
  }
  const c = container.getBoundingClientRect();
  const r = active.getBoundingClientRect();
  // Whole pixels, so edges stay crisp (and pixel art isn't smeared).
  const x = Math.round(r.left - c.left - container.clientLeft + container.scrollLeft);
  const y = Math.round(r.top - c.top - container.clientTop + container.scrollTop);
  const w = Math.round(r.right - c.left - container.clientLeft + container.scrollLeft) - x;
  const h = Math.round(r.bottom - c.top - container.clientTop + container.scrollTop) - y;
  const at = `${x},${y},${w},${h}`;
  if (el.dataset.at === at) return;
  const jump = instant || !el.dataset.at || reduced.matches;
  if (jump) el.style.transition = "none";
  else if (moved) void el.offsetWidth; // put back in the page: let it start from where it was
  el.style.transform = `translate(${x}px, ${y}px)`;
  el.style.width = `${w}px`;
  el.style.height = `${h}px`;
  el.classList.add("on");
  if (jump) {
    void el.offsetWidth;
    el.style.transition = "";
  }
  el.dataset.at = at;
}

/** Slide a panel in from the side (`dx`) or from above or below (`dy`) as it appears. */
export function slideIn(el: HTMLElement | null, dx: number, dy = 0) {
  if (!el || reduced.matches) return;
  el.animate(
    [
      { transform: `translate(${dx}px, ${dy}px)`, opacity: 0 },
      { transform: "none", opacity: 1 },
    ],
    { duration: 240, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
  );
}

document.fonts?.ready.then(resync);
