// Vite builds the Studio's web UI into `dist/`, which Tauri embeds (`src-tauri/tauri.conf.json`). In development
// `npm run tauri dev` starts this server and the Tauri window loads it.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Tauri expects a fixed port, and its own console output should stay visible.
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
