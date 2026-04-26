import { ReactNode } from "react";
import { motion, useInView } from "framer-motion";
import { useRef } from "react";

interface ScrollSectionProps {
  children: ReactNode;
  className?: string;
  delay?: number;
}

export default function ScrollSection({ children, className = "", delay = 0 }: ScrollSectionProps) {
  const ref = useRef<HTMLDivElement>(null);
  const isInView = useInView(ref, { once: true, margin: "-80px" });

  return (
    <motion.div
      ref={ref}
      initial={{ opacity: 0, y: 40 }}
      animate={isInView ? { opacity: 1, y: 0 } : {}}
      transition={{ duration: 0.7, delay, ease: [0.25, 0.46, 0.45, 0.94] }}
      className={className}
    >
      {children}
    </motion.div>
  );
}
