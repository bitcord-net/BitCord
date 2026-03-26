import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Each test scenario can take up to 60 s (P2P sync + message delivery).
    testTimeout: 60_000,
    hookTimeout: 30_000,
    // Run scenarios sequentially to avoid port conflicts between process-spawn tests.
    pool: "forks",
    poolOptions: {
      forks: { singleFork: true },
    },
    reporters: ["verbose"],
  },
});
