import { ReactNode } from "react";
import MonoLabel from "./MonoLabel";

// Left-aligned section header: mono kicker, display-family h2, optional lede.
export default function SectionHeader({
  kicker,
  title,
  lede,
}: {
  kicker: string;
  title: ReactNode;
  lede?: ReactNode;
}) {
  return (
    <div className="max-w-2xl">
      <MonoLabel>{kicker}</MonoLabel>
      <h2 className="font-display text-h2x font-semibold text-ink-hi mt-3">{title}</h2>
      {lede && <p className="text-bodyx text-ink-mid mt-4">{lede}</p>}
    </div>
  );
}
