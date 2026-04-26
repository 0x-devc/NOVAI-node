export default function EmptyState({ message }: { message: string }) {
  return (
    <div className="card text-sm text-slate-400 italic">{message}</div>
  );
}
