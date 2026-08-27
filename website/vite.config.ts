import { defineConfig } from "vite";
import react from "@vitejs/plugin-react-swc";
import path from "path";
import { componentTagger } from "lovable-tagger";

// https://vitejs.dev/config/
export default defineConfig(({ mode }) => ({
  server: {
    host: "::",
    port: 8080,
    hmr: {
      overlay: false,
    },
    // Dev-only live-data proxy (operator-approved Q15): lets the browser on
    // localhost reach the public RPC without CORS. The `server` block applies
    // to the dev server only; nothing here exists in the production build.
    proxy: {
      "/rpc": {
        target: "https://rpc.novai.network",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/rpc/, ""),
      },
    },
  },
  // Multi-page build. index.html MUST stay named here: the moment `input` is
  // set explicitly it stops being the implicit default, and omitting it would
  // silently drop the marketing site from the build.
  //
  // specimen.html is deliberately absent, which is what keeps it dev-only: the
  // dev server still serves it, the production build never sees it.
  build: {
    rollupOptions: {
      input: {
        main: path.resolve(__dirname, "index.html"),
        console: path.resolve(__dirname, "console.html"),
      },
    },
  },
  plugins: [react(), mode === "development" && componentTagger()].filter(Boolean),
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
}));
