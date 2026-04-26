import { useScroll, useTransform, MotionValue } from "framer-motion";

export function useParallax(range: number = 50): MotionValue<number> {
  const { scrollY } = useScroll();
  return useTransform(scrollY, [0, 500], [0, -range]);
}
