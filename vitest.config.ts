import { defineConfig } from "vitest/config";

// bench/ calls a real model and its cases hold source trees; only tests/ runs under vitest.
export default defineConfig({ test: { include: ["tests/**/*.test.ts"] } });
