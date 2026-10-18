import { defineConfig } from "vite";

// The page a phone, tablet or other computer opens (served by the app's
// local server, see src-tauri/src/server.rs). Plain TypeScript, no framework:
// small, and fast on older tablets. Built after the app into dist/receiver/,
// so the app build must not empty the folder afterwards.
export default defineConfig({
  root: "receiver",
  base: "/",
  publicDir: "public",
  build: {
    outDir: "../dist/receiver",
    emptyOutDir: true,
    // iPadOS 15 / Android Chrome of the last few years.
    target: ["safari15", "chrome100", "firefox100"],
  },
  server: { port: 1430, strictPort: true },
});
