import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const SCRIPT = resolve("scripts/generate-repo-stats.mjs");

function run(args: string[]): { status: number; output: string } {
  try {
    const out = execFileSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    return { status: 0, output: out };
  } catch (err) {
    const e = err as { status: number | null; stdout?: string; stderr?: string };
    return { status: e.status ?? -1, output: `${e.stdout ?? ""}${e.stderr ?? ""}` };
  }
}

describe("generate-repo-stats", () => {
  it("two consecutive runs against the unchanged tree are byte-identical", () => {
    const dir = mkdtempSync(join(tmpdir(), "repo-stats-"));
    const out = join(dir, "stats.json");
    expect(run(["--out", out]).status).toBe(0);
    const first = readFileSync(out, "utf8");
    expect(run(["--out", out]).status).toBe(0);
    const second = readFileSync(out, "utf8");
    expect(second).toBe(first);
  });

  it("produces the committed file exactly (staleness gate green)", () => {
    const committed = resolve("src/data/repo-stats.generated.json");
    const res = run(["--check", "--out", committed]);
    expect(res.output).toContain("check ok");
    expect(res.status).toBe(0);
  });

  it("fails loudly on a tree with zero .rs files", () => {
    const root = mkdtempSync(join(tmpdir(), "repo-stats-empty-"));
    mkdirSync(join(root, "crates", "foo"), { recursive: true });
    writeFileSync(join(root, "crates", "foo", "Cargo.toml"), "[package]\nname = \"foo\"\n");
    const out = join(root, "stats.json");
    const res = run(["--root", root, "--out", out]);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("zero .rs files");
  });

  it("--check fails on a doctored committed file", () => {
    const dir = mkdtempSync(join(tmpdir(), "repo-stats-doctor-"));
    const out = join(dir, "stats.json");
    cpSync(resolve("src/data/repo-stats.generated.json"), out);
    const doctored = readFileSync(out, "utf8").replace(/"value": 128811/, '"value": 999');
    writeFileSync(out, doctored);
    const res = run(["--check", "--out", out]);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale");
  });

  it("the committed values match the site's floor claims", () => {
    const stats = JSON.parse(readFileSync(resolve("src/data/repo-stats.generated.json"), "utf8"));
    expect(stats.linesOfRust.value).toBeGreaterThanOrEqual(100_000);
    expect(stats.tests.value).toBeGreaterThanOrEqual(2_100);
    expect(stats.crates.value).toBe(16);
    expect(stats.unsafeBlocks.value).toBe(0);
    expect(stats.txTypes.value).toBe(11);
    expect(stats.signalTypes.value).toBe(23);
    expect(stats.memoryObjectTypes.value).toBe(16);
  });
});
