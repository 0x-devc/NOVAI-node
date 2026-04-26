import { motion } from "framer-motion";

export default function GlowOrb({ className = "" }: { className?: string }) {
  return (
    <div className={`relative ${className}`}>
      {/* Outer cosmic glow */}
      <div
        className="absolute inset-[-60%] rounded-full animate-pulse-glow"
        style={{
          background: "radial-gradient(circle, rgba(76,111,255,0.25) 0%, rgba(125,211,252,0.10) 40%, transparent 70%)",
          filter: "blur(40px)",
        }}
      />
      {/* Middle nebula ring */}
      <div
        className="absolute inset-[-30%] rounded-full"
        style={{
          background: "radial-gradient(circle, rgba(125,211,252,0.18) 0%, rgba(76,111,255,0.08) 50%, transparent 70%)",
          filter: "blur(25px)",
        }}
      />
      {/* Extra glow layer */}
      <div
        className="absolute inset-[-15%] rounded-full"
        style={{
          background: "radial-gradient(circle, rgba(199,210,254,0.10) 0%, transparent 60%)",
          mixBlendMode: "screen",
          filter: "blur(15px)",
        }}
      />
      {/* Nova core */}
      <motion.div
        animate={{ scale: [1, 1.04, 1] }}
        transition={{ duration: 6, repeat: Infinity, ease: "easeInOut" }}
        className="relative w-full h-full rounded-full"
        style={{
          background: "radial-gradient(circle at 35% 35%, rgba(199,210,254,0.95), rgba(125,211,252,0.7) 30%, rgba(76,111,255,0.5) 60%, rgba(76,111,255,0.15) 100%)",
          boxShadow: "0 0 80px 20px rgba(76,111,255,0.35), 0 0 160px 60px rgba(125,211,252,0.15)",
        }}
      >
        {/* Star highlight */}
        <div
          className="absolute top-[18%] left-[22%] w-[25%] h-[25%] rounded-full"
          style={{
            background: "radial-gradient(circle, rgba(255,255,255,0.85), transparent 70%)",
            filter: "blur(6px)",
          }}
        />
        {/* Secondary glow spot */}
        <div
          className="absolute bottom-[25%] right-[20%] w-[18%] h-[18%] rounded-full"
          style={{
            background: "radial-gradient(circle, rgba(125,211,252,0.6), transparent 70%)",
            filter: "blur(8px)",
          }}
        />
      </motion.div>
      {/* Rotating energy ring */}
      <motion.div
        animate={{ rotate: 360 }}
        transition={{ duration: 30, repeat: Infinity, ease: "linear" }}
        className="absolute inset-[-10%]"
      >
        <div className="absolute top-0 left-1/2 w-2 h-2 rounded-full bg-accent/40" style={{ filter: "blur(2px)" }} />
        <div className="absolute bottom-0 left-1/2 w-1.5 h-1.5 rounded-full bg-primary/30" style={{ filter: "blur(2px)" }} />
        <div className="absolute top-1/2 left-0 w-1.5 h-1.5 rounded-full bg-accent/25" style={{ filter: "blur(2px)" }} />
        <div className="absolute top-1/2 right-0 w-2 h-2 rounded-full bg-primary/35" style={{ filter: "blur(2px)" }} />
      </motion.div>
    </div>
  );
}
