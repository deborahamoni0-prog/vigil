import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

// Three pages, as before: the landing page, the markets app and the live pipeline view.
export default defineConfig({
  plugins: [react()],
  server: {
    // In development, API calls go to the Rust gateway on port 3000.
    // VITE_GATEWAY_URL points the development server at another gateway. Localhost is the default.
    proxy: { "/api": process.env.VITE_GATEWAY_URL ?? "http://127.0.0.1:3000" },
  },
  build: {
    outDir: "dist",
    rollupOptions: {
      input: {
        index: resolve(__dirname, "index.html"),
        markets: resolve(__dirname, "markets.html"),
        live: resolve(__dirname, "live.html"),
      },
    },
  },
});
