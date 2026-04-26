import { motion, useScroll, useSpring } from "framer-motion";

export default function ScrollProgress() {
  const { scrollYProgress } = useScroll();
  const scaleX = useSpring(scrollYProgress, { stiffness: 100, damping: 30, restDelta: 0.001 });

  return (
    <motion.div
      style={{ scaleX, transformOrigin: "0%" }}
      className="fixed top-0 left-0 right-0 h-[2px] z-[60]"
    >
      <div
        className="w-full h-full"
        style={{
          background: "linear-gradient(90deg, hsl(228, 100%, 62%), hsl(192, 95%, 68%), hsl(270, 80%, 75%))",
        }}
      />
    </motion.div>
  );
}
