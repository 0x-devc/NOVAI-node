export default function ErrorState({ message }: { message: string }) {
  return (
    <div className="card border-red-900 bg-red-950/30">
      <p className="text-sm text-red-300">
        <span className="font-semibold">Error:</span> {message}
      </p>
    </div>
  );
}
