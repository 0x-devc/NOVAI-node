import { useEffect, useState, type RefObject } from "react";

// Continuous in-view tracking (not once). Drives the chain poll cadence:
// fast while the live section is actually on screen, slow otherwise.
// SSR-safe: false on the server and first paint.
export function useInView(ref: RefObject<Element>, margin = "0px"): boolean {
  const [inView, setInView] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") return;
    const obs = new IntersectionObserver(
      (entries) => setInView(entries.some((e) => e.isIntersecting)),
      { rootMargin: margin }
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [ref, margin]);
  return inView;
}
