import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The page must be opened at exactly this address: it is dev.toml's public_url, which the server
// checks the WebSocket's Origin against and which decides the cookie.
export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    proxy: { "/api": { target: "http://127.0.0.1:7777", ws: true } },
  },
  worker: { format: "es" },
  build: { target: "es2022" },
});
