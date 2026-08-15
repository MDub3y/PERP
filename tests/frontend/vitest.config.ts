import path from "node:path";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const here = path.dirname(fileURLToPath(import.meta.url));
// Frontend source lives one level up from the repo-root `tests/` directory
// this config sits in - see tests/README (this file's neighbor) for why
// tests are consolidated here instead of colocated in frontend/.
const frontendRoot = path.resolve(here, "../../frontend");

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": frontendRoot,
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: [path.resolve(here, "setup.ts")],
    include: [path.resolve(here, "**/*.test.{ts,tsx}")],
  },
});
