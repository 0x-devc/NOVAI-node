import { useState, FormEvent } from "react";
import { Link, NavLink, useNavigate } from "react-router-dom";
import { resolveSearch } from "../lib/search";

const navClass = ({ isActive }: { isActive: boolean }) =>
  `px-3 py-1.5 text-sm rounded-md transition-colors ${
    isActive
      ? "bg-slate-800 text-slate-100"
      : "text-slate-400 hover:text-slate-100"
  }`;

export default function Header() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (!query.trim()) return;
    setSearching(true);
    setError(null);
    try {
      const hit = await resolveSearch(query);
      if (!hit) {
        setError("Not a height, hash, or address");
        return;
      }
      switch (hit.kind) {
        case "block":
          navigate(`/blocks/${hit.height}`);
          break;
        case "tx":
          navigate(`/tx/${hit.txid}`);
          break;
        case "address":
          navigate(`/account/${hit.address}`);
          break;
      }
      setQuery("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSearching(false);
    }
  }

  return (
    <header className="border-b border-slate-800 bg-slate-950/80 backdrop-blur sticky top-0 z-10">
      <div className="mx-auto w-full max-w-6xl px-4 py-3 flex items-center gap-4">
        <Link
          to="/"
          className="text-slate-100 font-semibold text-lg tracking-tight"
        >
          NOVAI <span className="text-sky-400">Explorer</span>
        </Link>

        <form onSubmit={onSubmit} className="flex-1 max-w-xl ml-4">
          <div className="relative">
            <input
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Block height, hash, txid, address, or entity id"
              className="w-full bg-slate-900 border border-slate-700 focus:border-sky-500 focus:outline-none rounded-md px-3 py-1.5 text-sm placeholder:text-slate-500 font-mono"
              spellCheck={false}
              autoComplete="off"
            />
            {searching && (
              <span className="absolute right-3 top-1/2 -translate-y-1/2 text-xs text-slate-500">
                searching…
              </span>
            )}
          </div>
          {error && (
            <p className="text-xs text-red-400 mt-1 px-1">{error}</p>
          )}
        </form>

        <nav className="flex items-center gap-1">
          <NavLink to="/blocks" className={navClass}>
            Blocks
          </NavLink>
          <NavLink to="/stats" className={navClass}>
            Stats
          </NavLink>
        </nav>
      </div>
    </header>
  );
}
