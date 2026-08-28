import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, cpSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";

const SCRIPT = resolve("scripts/generate-console-data.mjs");
const DASH_GATE = resolve("scripts/check-dashes.mjs");
const REPO = resolve("..");
const DATA = resolve("src/data/console-data.generated.json");
const OPENRPC = resolve("public/openrpc.json");

function run(script: string, args: string[]): { status: number; output: string } {
  try {
    const out = execFileSync(process.execPath, [script, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
    return { status: 0, output: out };
  } catch (err) {
    const e = err as { status: number | null; stdout?: string; stderr?: string };
    return { status: e.status ?? -1, output: `${e.stdout ?? ""}${e.stderr ?? ""}` };
  }
}

// The generator reads a handful of files from the repo root. A fixture root
// copies exactly those, so a doctored copy proves a gate fires without touching
// the real tree. The crates/ walk (the "only one dispatch site" assertion) sees
// only the copied files, which is what keeps this cheap.
const FIXTURE_FILES = [
  "docs/RPC_REFERENCE.md",
  "README.md",
  "crates/node/src/rpc.rs",
  "crates/execution/src/lib.rs",
  "crates/consensus/src/lib.rs",
  "crates/ai_entities/src/signals.rs",
  "crates/ai_entities/src/memory.rs",
  "sdk/novai-python-sdk/novai_sdk/client.py",
];

function fixtureRoot(edit?: (rel: string, text: string) => string): string {
  const root = mkdtempSync(join(tmpdir(), "console-data-"));
  for (const rel of FIXTURE_FILES) {
    const dest = join(root, rel);
    mkdirSync(dirname(dest), { recursive: true });
    const text = readFileSync(join(REPO, rel), "utf8");
    writeFileSync(dest, edit ? edit(rel, text) : text);
  }
  return root;
}

function generate(root: string): { status: number; output: string } {
  const out = join(root, "out.json");
  const rpc = join(root, "openrpc.json");
  return run(SCRIPT, ["--root", root, "--out", out, "--openrpc", rpc]);
}

const data = JSON.parse(readFileSync(DATA, "utf8"));
const openrpc = JSON.parse(readFileSync(OPENRPC, "utf8"));

describe("generate-console-data: determinism and the staleness gate", () => {
  it("two consecutive runs against the unchanged tree are byte-identical", () => {
    const dir = mkdtempSync(join(tmpdir(), "console-data-det-"));
    const out = join(dir, "d.json");
    const rpc = join(dir, "r.json");
    expect(run(SCRIPT, ["--out", out, "--openrpc", rpc]).status).toBe(0);
    const firstData = readFileSync(out, "utf8");
    const firstRpc = readFileSync(rpc, "utf8");
    expect(run(SCRIPT, ["--out", out, "--openrpc", rpc]).status).toBe(0);
    expect(readFileSync(out, "utf8")).toBe(firstData);
    expect(readFileSync(rpc, "utf8")).toBe(firstRpc);
  });

  it("--check is green against both committed files", () => {
    const res = run(SCRIPT, ["--check"]);
    expect(res.output).toContain("check ok");
    expect(res.status).toBe(0);
  });

  it("--check fails on a doctored console-data file", () => {
    const dir = mkdtempSync(join(tmpdir(), "console-data-doctor-"));
    const out = join(dir, "d.json");
    cpSync(DATA, out);
    writeFileSync(out, readFileSync(out, "utf8").replace("novai_getLatestBlock", "novai_getLatestBlockX"));
    const res = run(SCRIPT, ["--check", "--out", out, "--openrpc", OPENRPC]);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale");
  });

  it("--check fails on a doctored openrpc file", () => {
    const dir = mkdtempSync(join(tmpdir(), "console-data-doctor-rpc-"));
    const rpc = join(dir, "r.json");
    cpSync(OPENRPC, rpc);
    writeFileSync(rpc, readFileSync(rpc, "utf8").replace('"openrpc": "1.2.6"', '"openrpc": "9.9.9"'));
    const res = run(SCRIPT, ["--check", "--out", DATA, "--openrpc", rpc]);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale");
  });
});

describe("generate-console-data: the four-way drift gate", () => {
  it("passes today, with all four sources naming the same 29 methods", () => {
    expect(data.drift.value.disagreements).toEqual([]);
    expect(data.drift.value.agreedMethodCount).toBe(29);
    expect(data.drift.value.sources).toHaveLength(4);
    for (const s of data.drift.value.sources) expect(s.count).toBe(29);
  });

  it("fails when the doc gains a method the implementation does not have", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace("## Error codes", "### `novai_getInvented`\n\nInvented.\n\n**Example**:\n\n```bash\ncurl -d '{\"method\":\"novai_getInvented\"}'\n```\n\n## Error codes")
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("novai_getInvented");
  });

  it("fails when a dispatch arm is removed from rpc.rs", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "crates/node/src/rpc.rs"
        ? text.replace('"novai_getNonce" =>', '"novai_getNonceRenamed" =>')
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("drift gate");
  });

  it("fails when the README method table loses a row", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "README.md"
        ? text.split("\n").filter((l) => !l.startsWith("| `novai_getBalance`")).join("\n")
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("novai_getBalance");
  });

  it("does not count a method that appears only in a doc comment", () => {
    // rpc.rs refers to novai_getStatus in three doc comments. A method that
    // exists only in prose must never reach the dispatch name set.
    const rpc = readFileSync(join(REPO, "crates/node/src/rpc.rs"), "utf8");
    expect(rpc).toContain("novai_getStatus");
    expect(data.drift.value.sources.find((s: { key: string }) => s.key === "rpc.rs dispatch").count).toBe(29);
    expect(JSON.stringify(data.methods.value)).not.toContain("novai_getStatus");
  });

  it("does not count a method that appears only in a Python docstring", () => {
    let doctored = "";
    const root = fixtureRoot((rel, text) => {
      if (rel !== "sdk/novai-python-sdk/novai_sdk/client.py") return text;
      doctored = text.replace('"""', '"""Calls "novai_getInventedBySdk" somewhere in prose.\n\n');
      return doctored;
    });
    // Prove the doctoring landed, so a passing run cannot mean the edit was a
    // no-op rather than the scanner doing its job.
    expect(doctored).toContain("novai_getInventedBySdk");
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.output).not.toContain("novai_getInventedBySdk");
    expect(res.status).toBe(0);
  });
});

describe("generate-console-data: the KNOWN_DRIFT exception list", () => {
  it("carries exactly the exceptions that still apply, each with an operator reference", () => {
    const ids = data.drift.value.knownExceptions.map((e: { id: string }) => e.id).sort();
    expect(ids).toEqual([
      "error-code-32014-undocumented",
      "faucet-disabled-code-mismatch",
      "faucet-rpc-gating-incomplete",
      "public-faucet-gating-backwards",
    ]);
    for (const e of data.drift.value.knownExceptions) {
      expect(e.operatorRef).toMatch(/NEEDS-OPERATOR\.md item \d+/);
      expect(e.why.length).toBeGreaterThan(40);
    }
  });

  it("prints every exception on a successful run, not only on failure", () => {
    const res = run(SCRIPT, ["--check"]);
    expect(res.status).toBe(0);
    for (const e of data.drift.value.knownExceptions) expect(res.output).toContain(e.id);
  });

  it("fails, naming the entry to delete, when an exception stops applying", () => {
    // Document -32014 and the first exception has nothing left to suppress.
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace(
            "| `-32013` | Other validation error",
            "| `-32014` | Nonce too high | tx nonce is above the horizon |\n| `-32013` | Other validation error"
          )
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("no longer applies");
    expect(res.output).toContain("error-code-32014-undocumented");
    expect(res.output).toContain("Remove it from KNOWN_DRIFT");
  });

  it("fails when the novai_faucet gating exception is fixed in the doc", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace(
            "Available **only** when the node was launched with `--dev-keys --allow-insecure-dev-keys`",
            "Available when the node was launched with `--faucet-key`, or with `--dev-keys --allow-insecure-dev-keys`"
          )
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("faucet-rpc-gating-incomplete");
    expect(res.output).toContain("no longer applies");
  });

  it("fails when the public faucet gating exception is fixed in the doc", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace(
            "is available only when the node is launched in Dev-mode",
            "is available only when the node is launched with --faucet-key"
          )
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("public-faucet-gating-backwards");
  });
});

describe("generate-console-data: the document grammar traps", () => {
  it("reads novai_faucet's Params table, not the Cooldowns table above it", () => {
    const faucet = data.methods.value.find((m: { name: string }) => m.name === "novai_faucet");
    expect(faucet.params.list.map((p: { field: string }) => p.field)).toEqual(["address"]);
  });

  it("keeps duplicate error codes, because errors are a list and never a map", () => {
    const faucet = data.methods.value.find((m: { name: string }) => m.name === "novai_faucet");
    const codes = faucet.errors.list.map((e: { code: number }) => e.code);
    expect(codes).toEqual([-32602, -32000, -32000]);
    const rpcFaucet = openrpc.methods.find((m: { name: string }) => m.name === "novai_faucet");
    expect(rpcFaucet.errors.filter((e: { code: number }) => e.code === -32000)).toHaveLength(2);
  });

  it("treats a label with its period inside the bold as prose, not as a field", () => {
    // "**Building the hex.**" sits between the summary and Params in
    // novai_submitTransaction. It must not be absorbed into either.
    const submit = data.methods.value.find((m: { name: string }) => m.name === "novai_submitTransaction");
    expect(submit.description).not.toContain("Building the hex");
    expect(submit.params.list.map((p: { field: string }) => p.field)).toEqual(["tx"]);
  });

  it("resolves every alias to a concrete shape", () => {
    const aliased = data.methods.value.filter(
      (m: { params?: { resolvedFrom?: string }; result?: { resolvedFrom?: string }; errors?: { resolvedFrom?: string } }) =>
        m.params?.resolvedFrom || m.result?.resolvedFrom || m.errors?.resolvedFrom
    );
    expect(aliased).toHaveLength(7);
    for (const m of aliased) {
      if (m.params?.resolvedFrom) expect(m.params.list.length).toBeGreaterThan(0);
      if (m.result?.resolvedFrom) expect(m.result.envelope).toBeTruthy();
      if (m.errors?.resolvedFrom) expect(m.errors.list.length).toBeGreaterThan(0);
    }
  });

  it("attaches the category record shape to every envelope that names a record type", () => {
    const inheriting = data.methods.value.filter(
      (m: { result?: { kind: string; recordShape?: string } }) =>
        m.result && (m.result.kind === "categoryResult" || m.result.recordShape)
    );
    expect(inheriting).toHaveLength(17);
    for (const m of inheriting) {
      const shape = m.result.kind === "categoryResult" ? m.result.envelope : m.result.recordShape;
      expect(shape).toBeTruthy();
      expect(shape.length).toBeGreaterThan(20);
    }
  });

  it("leaves no method without params, a result and errors after resolution", () => {
    for (const m of data.methods.value) {
      expect(m.params, `${m.name} params`).toBeTruthy();
      expect(m.result, `${m.name} result`).toBeTruthy();
      expect(m.errors, `${m.name} errors`).toBeTruthy();
      expect(m.curl, `${m.name} curl`).toContain(m.name);
      expect(m.brief, `${m.name} brief`).toBeTruthy();
    }
  });

  it("fails when a curl example calls a different method than its heading", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace('"method":"novai_getNonce"', '"method":"novai_getBalance"')
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("is for a different method");
  });

  it("fails when the method index disagrees with the method sections", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace("| | [`novai_getNonce`](#novai_getnonce) | Account expected nonce |\n", "")
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("method index disagrees");
  });
});

describe("generate-console-data: dash normalisation is proven, not assumed", () => {
  // Built from code points rather than written as literals, so this file does
  // not itself become a place the dash gate has to be told to ignore.
  const FORBIDDEN_POINTS = [0x2014, 0x2013, 0x2012, 0x2015, 0x2212];
  const FORBIDDEN = FORBIDDEN_POINTS.map((p) => String.fromCodePoint(p));
  const MINUS = String.fromCodePoint(0x2212);

  it("emits no forbidden code point into either generated file", () => {
    for (const file of [DATA, OPENRPC]) {
      const text = readFileSync(file, "utf8");
      FORBIDDEN.forEach((c, i) => {
        expect(text.includes(c), `${file} contains U+${FORBIDDEN_POINTS[i].toString(16)}`).toBe(false);
      });
    }
  });

  it("logs one substitution for every forbidden code point in the sources", () => {
    const doc = readFileSync(join(REPO, "docs/RPC_REFERENCE.md"), "utf8");
    const present = [...doc].filter((c) => FORBIDDEN.includes(c)).length;
    const res = run(SCRIPT, ["--check"]);
    const logged = res.output.split("\n").filter((l) => l.includes("console-data: normalised")).length;
    expect(logged).toBe(present);
  });

  it("the dash gate reports a violation when one is present", () => {
    // A gate that has never been seen to fail is not a gate. This injects the
    // exact character the reference contains into a scanned file.
    const probe = resolve("src/dash-gate-probe.tmp.md");
    try {
      writeFileSync(probe, `range check: end ${MINUS} start\n`);
      const res = run(DASH_GATE, []);
      expect(res.status).not.toBe(0);
      expect(res.output).toContain("minus sign (U+2212)");
    } finally {
      rmSync(probe, { force: true });
    }
    expect(run(DASH_GATE, []).status).toBe(0);
  });
});

describe("generate-console-data: the derived enumerations", () => {
  it("carries the chain's 23 signal types, 16 memory object types and 11 tx types", () => {
    expect(data.signalTypes.value).toHaveLength(23);
    expect(data.memoryObjectTypes.value).toHaveLength(16);
    expect(data.txTypes.value).toHaveLength(11);
  });

  it("names the signal types that have no description in the source", () => {
    const missing = data.gaps.value.signalTypesWithoutSourceDescription;
    expect(missing).toEqual([
      "Anomaly", "Optimization", "Prediction", "RiskScore",
      "AuditReport", "SpamRisk", "CongestionForecast",
    ]);
    for (const s of data.signalTypes.value) {
      // Every type carries its payload note even when the prose is missing, so
      // the page can render a complete table and flag only the prose gap.
      expect(s.payloadNote, s.variant).toBeTruthy();
      if (missing.includes(s.variant)) expect(s.description).toBeNull();
      else expect(s.description).toBeTruthy();
    }
  });

  it("publishes the retention window in blocks, never in wall-clock time", () => {
    expect(data.networkParameters.value.blockRetention.blocks).toBe(50_000);
    expect(JSON.stringify(data.networkParameters)).not.toMatch(/hours|minutes|blocks per second|bps/i);
  });

  it("wraps every top-level dataset with the method that produced it", () => {
    for (const [key, entry] of Object.entries(data)) {
      if (key === "generatedAt" || key === "source") continue;
      expect((entry as { method?: string }).method, key).toBeTruthy();
      expect(entry, key).toHaveProperty("value");
    }
  });
});

describe("openrpc.json", () => {
  it("describes all 29 methods and declares its own provenance", () => {
    expect(openrpc.openrpc).toBe("1.2.6");
    expect(openrpc.methods).toHaveLength(29);
    expect(openrpc.info.description).toContain("Generated from");
  });

  it("marks optional params optional and required params required", () => {
    const byTag = openrpc.methods.find((m: { name: string }) => m.name === "novai_getOracleAnchorsByTag");
    const required = byTag.params.filter((p: { required: boolean }) => p.required).map((p: { name: string }) => p.name);
    const optional = byTag.params.filter((p: { required: boolean }) => !p.required).map((p: { name: string }) => p.name);
    expect(required).toContain("data_tag");
    expect(optional).toEqual(["ts_min", "ts_max"]);
  });

  it("keeps the documented type alongside the JSON schema type", () => {
    const latest = openrpc.methods.find((m: { name: string }) => m.name === "novai_getBlockByHash");
    expect(latest.params[0].schema["x-novai-doc-type"]).toBe("hex32");
    expect(latest.params[0].schema.pattern).toBe("^[0-9a-fA-F]{64}$");
  });

  it("fails rather than guessing when the doc introduces an unmapped type", () => {
    const root = fixtureRoot((rel, text) =>
      rel === "docs/RPC_REFERENCE.md"
        ? text.replace("| `hash` | `hex32` | exactly 64 hex chars |", "| `hash` | `bytes48` | exactly 96 hex chars |")
        : text
    );
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("no schema mapping");
  });
});
