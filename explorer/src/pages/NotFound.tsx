import { Link } from "react-router-dom";

export default function NotFound() {
  return (
    <div className="card text-center py-12">
      <h1 className="text-2xl font-semibold">Page not found</h1>
      <p className="mt-2 text-slate-400">
        Try the{" "}
        <Link to="/blocks" className="text-sky-300 hover:underline">
          latest blocks
        </Link>{" "}
        or use the search bar above.
      </p>
    </div>
  );
}
