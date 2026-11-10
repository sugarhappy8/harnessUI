import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

// Tauri expects a fixed port and fails if it is not available.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: [
      {
        // The markdown parser decodes entities such as &amp; with this package. Its browser
        // build assigns a string to innerHTML, which the CSP's
        // require-trusted-types-for 'script' refuses in WebView2; the plain build uses a
        // lookup table and touches no DOM.
        find: /^decode-named-character-reference$/,
        replacement: fileURLToPath(
          new URL("./node_modules/decode-named-character-reference/index.js", import.meta.url),
        ),
      },
    ],
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  build: {
    target: ["es2022", "chrome120", "safari16"],
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        overlay: fileURLToPath(new URL("./overlay.html", import.meta.url)),
        indicator: fileURLToPath(new URL("./indicator.html", import.meta.url)),
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
