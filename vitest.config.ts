import { defineConfig } from "vitest/config";

// bench/ calls a real model and its cases hold source trees; only tests/ runs under vitest.
// The Keychain service is renamed so no test can read or write the operator's real TypeSafe key.
export default defineConfig({ test: { include: ["tests/**/*.test.ts"], env: { PR_REVIEW_KEYCHAIN_SERVICE: "io.github.diego-arbio-dev.pr-review.vitest-absent" } } });
