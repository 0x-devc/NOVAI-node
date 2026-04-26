import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Vite proxies /rpc → the local NOVAI node. Override with NOVAI_RPC_URL env var
// if your node listens elsewhere (e.g. NOVAI_RPC_URL=http://my-node:3030).
const RPC_TARGET = process.env.NOVAI_RPC_URL ?? "http://localhost:3030";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      "/rpc": {
        target: RPC_TARGET,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/rpc/, ""),
      },
    },
  },
});
