import { describe, it, expect } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import repoStats from "@/data/repo-stats.generated.json";
import { floorTo, flooredClaim } from "@/lib/format";
import { STATS } from "@/data/stats";
import SinglePage from "@/pages/SinglePage";

describe("derived stats", () => {
  it("formatter floors to the step and appends the plus", () => {
    expect(floorTo(2138, 100)).toBe(2100);
    expect(floorTo(128811, 1000)).toBe(128000);
    expect(flooredClaim(128811, 1000)).toBe("128,000+");
  });

  it("STATS derives the tests value from the generated JSON", () => {
    const tests = STATS.find((s) => s.label === "Tests");
    expect(tests?.value).toBe(floorTo(repoStats.tests.value, 100));
    expect(STATS.find((s) => s.label === "Validators")?.value).toBe(4);
    expect(STATS.some((s) => /blocks/i.test(s.label))).toBe(false);
  });

  it("no hand-typed metric literals remain in the page or data source", () => {
    const page = readFileSync(resolve("src/pages/SinglePage.tsx"), "utf8");
    expect(page).not.toMatch(/65,?000|128,?000|\b2,?100\b|\b1,?100\b|30M/);
    const data = readFileSync(resolve("src/data/stats.ts"), "utf8");
    expect(data).toContain("repo-stats.generated.json");
    expect(data).not.toMatch(/\b2,?100\b|\b1,?100\b/);
  });

  it("renders the socials line and the tests stat from the JSON", async () => {
    render(<SinglePage />);
    const linesClaim = `${flooredClaim(repoStats.linesOfRust.value, 1000)} lines of Rust`;
    expect(
      screen.getByText((text) => text.includes(linesClaim))
    ).toBeInTheDocument();
    const testsClaim = `${floorTo(repoStats.tests.value, 100).toLocaleString()}+`;
    await waitFor(
      () => expect(screen.getByText(testsClaim)).toBeInTheDocument(),
      { timeout: 5000 }
    );
  });
});
