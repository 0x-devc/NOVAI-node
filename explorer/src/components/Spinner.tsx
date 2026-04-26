export default function Spinner({ label = "Loading…" }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-slate-400 text-sm">
      <span className="inline-block h-3 w-3 rounded-full bg-sky-400 animate-pulse" />
      {label}
    </div>
  );
}
