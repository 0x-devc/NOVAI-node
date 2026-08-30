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
      // Every console page is a real entry. index.html must stay named: the
      // moment `input` is set it stops being the implicit default, and omitting
      // it silently deletes the marketing site from the build.
      //
      // all.html and names.html are generated whole by
      // scripts/generate-console-html.mjs. They are listed here because they
      // are pages a reader loads, so they need the stylesheet link Vite injects
      // like any other entry.
      input: {
        main: path.resolve(__dirname, "index.html"),
        console: path.resolve(__dirname, "console.html"),
        consoleRpc: path.resolve(__dirname, "console/rpc.html"),
        consoleErrors: path.resolve(__dirname, "console/errors.html"),
        consoleTransactions: path.resolve(__dirname, "console/transactions.html"),
        consoleEntities: path.resolve(__dirname, "console/entities.html"),
        consoleSdks: path.resolve(__dirname, "console/sdks.html"),
        consoleNetwork: path.resolve(__dirname, "console/network.html"),
        consoleVerify: path.resolve(__dirname, "console/verify.html"),
        consoleAll: path.resolve(__dirname, "console/all.html"),
        consoleNames: path.resolve(__dirname, "console/names.html"),
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
