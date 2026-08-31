import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";

// The frozen snapshots are the artifact the adversarial pass reads, so they are
// the one thing that must not be able to drift from the pages while the suite
// stays green. Until this file existed, freeze-console --check ran only in
// prebuild and predeploy, so npm test could pass on a snapshot describing a page
// that no longer existed. Every finding of the second adversarial run was made
// against an artifact nothing in the suite had gated.

const SCRIPT = resolve("scripts/freeze-console.mjs");

const PAGES = [
  "console.html",
  "console/rpc.html",
  "console/errors.html",
  "console/transactions.html",
  "console/entities.html",
  "console/sdks.html",
  "console/network.html",
  "console/verify.html",
  "console/all.html",
  "console/names.html",
];

const snapshotFor = (rel: string) => rel.replace(/\//g, "__").replace(/\.html$/, ".txt");

function run(args: string[]): { status: number; output: string } {
  try {
    const out = execFileSync(process.execPath, [SCRIPT, ...args], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { status: 0, output: out };
  } catch (err) {
    const e = err as { status: number | null; stdout?: string; stderr?: string };
    return { status: e.status ?? -1, output: `${e.stdout ?? ""}${e.stderr ?? ""}` };
  }
}

/**
 * The whole page set and its snapshots, copied to a temp root and then doctored.
 *
 * The working tree is never written to. `edit` returns null to leave a file
 * alone, and the caller asserts through `landed` that its edit actually applied:
 * a String.replace whose anchor has drifted is a no-op, and a no-op probe makes
 * this test pass for the wrong reason.
 */
function withTree(edit: (rel: string, text: string) => string | null): { status: number; output: string } {
  const dir = mkdtempSync(join(tmpdir(), "freeze-console-"));
  try {
    mkdirSync(join(dir, "snapshots"), { recursive: true });
    for (const rel of PAGES) {
      const pageText = readFileSync(resolve(rel), "utf8");
      const pageTarget = join(dir, rel);
      mkdirSync(dirname(pageTarget), { recursive: true });
      writeFileSync(pageTarget, edit(rel, pageText) ?? pageText);

      const snapRel = `snapshots/${snapshotFor(rel)}`;
      const snapText = readFileSync(resolve(snapRel), "utf8");
      writeFileSync(join(dir, snapRel), edit(snapRel, snapText) ?? snapText);
    }
    return run(["--check", "--web-root", dir]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("freeze-console", () => {
  it("the committed snapshots match the committed pages", () => {
    const res = run(["--check"]);
    expect(res.status, res.output).toBe(0);
    expect(res.output).toContain(`${PAGES.length} snapshots match the pages`);
  });

  it("an untouched copy of the tree still passes, so the harness itself is sound", () => {
    // Without this, a harness that silently mangled every file would make the
    // two failure probes below pass no matter what the gate does.
    const res = withTree(() => null);
    expect(res.status, res.output).toBe(0);
  });

  it("fails when a page changes and its snapshot does not", () => {
    let landed = false;
    const res = withTree((rel, text) => {
      if (rel !== "console/network.html") return null;
      const out = text.replace("Retention is published in blocks", "Retention is published in furlongs");
      landed = out.includes("furlongs");
      return out;
    });
    expect(landed, "the page edit did not apply, so the gate was never given anything to catch").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale: console/network.html");
    expect(res.output).toContain("1 snapshot(s) are stale");
  });

  it("fails when a snapshot is edited away from its page", () => {
    // The other direction: the page is untouched and the snapshot is doctored.
    // A check that only ever re-rendered would miss this.
    let landed = false;
    const res = withTree((rel, text) => {
      if (rel !== "snapshots/console__verify.txt") return null;
      const out = `a line that is on no page\n${text}`;
      landed = out !== text;
      return out;
    });
    expect(landed, "the snapshot edit did not apply").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale: console/verify.html");
  });

  it("fails loudly when a snapshot is missing rather than silently writing one", () => {
    const dir = mkdtempSync(join(tmpdir(), "freeze-console-"));
    try {
      mkdirSync(join(dir, "snapshots"), { recursive: true });
      for (const rel of PAGES) {
        const target = join(dir, rel);
        mkdirSync(dirname(target), { recursive: true });
        writeFileSync(target, readFileSync(resolve(rel), "utf8"));
      }
      const res = run(["--check", "--web-root", dir]);
      expect(res.status).not.toBe(0);
      expect(res.output).toContain("does not exist; run npm run console:freeze");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("leaves numeric entities undecoded, which is what the adversarial reader sees", () => {
    // Deliberate and load-bearing, not an oversight. console.html must stay ASCII
    // for the dash gate, so a real em dash is written as an entity; decoding it
    // here would put a forbidden code point into a file the dash gate walks.
    // Pinned because an adversarial pass reads these snapshots and has to know
    // that "&#8212;" on this line is an em dash on the page.
    const landing = readFileSync(resolve("snapshots/console.txt"), "utf8");
    expect(landing).toMatch(/&#\d+;/);
    // Built from code points rather than written literally, because this file is
    // itself walked by the dash gate and is not a declared definition site.
    const forbidden = new RegExp(`[${[0x2012, 0x2013, 0x2014, 0x2015, 0x2212].map((c) => String.fromCodePoint(c)).join("")}]`);
    for (const rel of PAGES) {
      const snap = readFileSync(resolve(`snapshots/${snapshotFor(rel)}`), "utf8");
      expect(forbidden.test(snap), `${rel} snapshot carries a forbidden dash code point`).toBe(false);
    }
  });
});
