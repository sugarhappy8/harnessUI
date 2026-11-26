// Preview harness: serves the real app (src/) in a plain browser with Tauri's IPC mocked, for
// the before/after screenshots of Studio (README.md). Not part of the app: the app's tsconfig,
// index.html, build and tests never reach this folder.
//
// The Vite root is the repository itself, so `src/` resolves exactly as in `npm run dev`, and
// Tailwind scans the same files (its scan base is Vite's root). The page is /e2e/preview/.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../..", import.meta.url));
const port = Number(process.env.PREVIEW_PORT ?? 5199);

export default defineConfig({
  root: repo,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      {
        // Same alias as the app's vite.config.ts (the markdown parser's DOM-free build).
        find: /^decode-named-character-reference$/,
        replacement: `${repo}node_modules/decode-named-character-reference/index.js`,
      },
    ],
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**", "**/.git/**"] },
  },
});
