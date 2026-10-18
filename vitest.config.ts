import { defineConfig } from "vitest/config";

// Unit tests for the pure TS parts (receiver maths, protocol, names); no
// browser or Tauri needed.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "receiver/src/**/*.test.ts"],
  },
});
