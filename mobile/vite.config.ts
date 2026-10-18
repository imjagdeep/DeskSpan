import { defineConfig } from "vite";

// The DeskSpan Screen app: index.html finds the computer (QR / address /
// recent), screen.html is the same receiver page the browser gets
// (receiver/), opened with ?host=<computer>.
export default defineConfig({
  publicDir: "../receiver/public",
  clearScreen: false,
  server: { port: 1440, strictPort: true },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: ["safari15", "chrome100"],
    rollupOptions: { input: { index: "index.html", screen: "screen.html" } },
  },
});
