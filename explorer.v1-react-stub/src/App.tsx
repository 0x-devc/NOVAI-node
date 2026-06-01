import { Outlet } from "react-router-dom";
import Header from "./components/Header";

export default function App() {
  return (
    <div className="min-h-screen flex flex-col">
      <Header />
      <main className="mx-auto w-full max-w-6xl px-4 py-6 flex-1">
        <Outlet />
      </main>
      <footer className="mx-auto w-full max-w-6xl px-4 py-6 text-xs text-slate-500">
        NOVAI Explorer · talks to the local node via{" "}
        <code className="hex">/rpc</code> (proxied to{" "}
        <code className="hex">localhost:3030</code> in dev)
      </footer>
    </div>
  );
}
