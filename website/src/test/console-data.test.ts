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
  "crates/node/src/main.rs",
  "crates/node/src/snapshot/valset.rs",
  "crates/execution/src/lib.rs",
  "crates/consensus/src/lib.rs",
  "crates/consensus_types/src/leader.rs",
  "crates/codec/src/lib.rs",
  "crates/types/src/lib.rs",
  "crates/ai_entities/src/lib.rs",
  "crates/ai_entities/src/signals.rs",
  "crates/ai_entities/src/memory.rs",
  "sdk/novai-python-sdk/novai_sdk/client.py",
  "sdk/novai-python-sdk/novai_sdk/enums.py",
  "sdk/novai-python-sdk/novai_sdk/tx/transfer.py",
  "sdk/novai-python-sdk/novai_sdk/tx/signal.py",
  "sdk/novai-python-sdk/novai_sdk/tx/memory.py",
  "sdk/novai-python-sdk/novai_sdk/tx/governance.py",
  "sdk/novai-python-sdk/novai_sdk/tx/entities.py",
  "sdk/novai-sdk/Cargo.toml",
  "sdk/novai-sdk/src/lib.rs",
  "sdk/novai-sdk/src/tx.rs",
  "sdk/novai-sdk-ts/src/tx.ts",
  "sdk/novai-sdk-ts/src/types.ts",
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
      "blockbyheight-null-called-unreachable",
      "error-code-32014-undocumented",
      "faucet-disabled-code-mismatch",
      "faucet-rpc-gating-incomplete",
      "getnonce-documented-as-interchangeable",
      "getnonce-inherits-unreachable-db-error",
      "invalid-request-trigger-is-wrong",
      "latestblock-claims-only-global-errors",
      "public-faucet-gating-backwards",
      "sla-seller-cap-does-not-exist",
      "vk-list-error-clause-names-foreign-field",
    ]);
    for (const e of data.drift.value.knownExceptions) {
      expect(e.operatorRef).toMatch(/NEEDS-OPERATOR\.md item \d+/);
      expect(e.why.length).toBeGreaterThan(40);
    }
    // Every exception that names methods must land on them, and only on them.
    // The list can only shrink, so an id added here without a NEEDS-OPERATOR
    // entry and a measured predicate is a test edit rather than a decision.
    const labelled = new Set<string>();
    for (const m of data.methods.value) for (const c of m.caveats) labelled.add(c.exceptionId);
    for (const id of labelled) expect(ids).toContain(id);
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

  it("accounts for every forbidden code point in the reference document", () => {
    // The log groups by file and code point, because reading crates/ pulls in
    // source whose comments carry em dashes and one line per occurrence buried
    // the drift report. The proof is unchanged in substance: the count the
    // generator reports for a file must equal the count actually in that file.
    const rel = "docs/RPC_REFERENCE.md";
    const doc = readFileSync(join(REPO, rel), "utf8");
    const present = [...doc].filter((c) => FORBIDDEN.includes(c)).length;
    const res = run(SCRIPT, ["--check"]);
    const lines = res.output.split("\n").filter((l) => l.includes("console-data: normalised") && l.includes(rel));
    const reported = lines.reduce((total, line) => {
      const many = /normalised (\d+) x /.exec(line);
      return total + (many ? Number(many[1]) : 1);
    }, 0);
    expect(reported, `expected ${rel} to account for ${present} substitution(s)`).toBe(present);
  });

  it("reports a count for every source it normalises, so the log cannot be vacuous", () => {
    const res = run(SCRIPT, ["--check"]);
    const lines = res.output.split("\n").filter((l) => l.includes("console-data: normalised"));
    expect(lines.length).toBeGreaterThan(0);
    for (const line of lines) {
      // Every line names a file and a source location, grouped or not.
      expect(line, `substitution line carries no location: ${line}`).toMatch(/[\w./-]+:\d+:\d+/);
    }
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

// ---------------------------------------------------------------------------
// The source-derived datasets
//
// Every gate below is proven by doctoring a fixture, and every doctoring is
// itself asserted to have landed. A gate that has never been seen to fail has
// not been shown to work, and a doctoring that silently did nothing makes a
// passing run mean nothing.
// ---------------------------------------------------------------------------

describe("generate-console-data: error codes read from the implementation", () => {
  it("carries all thirteen codes the node can emit, each with a source line", () => {
    const codes = data.errorCodes.value.map((e: { code: number }) => e.code);
    expect(codes).toHaveLength(13);
    expect(codes).toContain(-32014);
    for (const e of data.errorCodes.value) {
      expect(e.file).toBe("crates/node/src/rpc.rs");
      expect(e.line).toBeGreaterThan(0);
    }
  });

  it("finds the codes emitted as tuple arms, not only the RpcError literals", () => {
    // -32010 through -32014 exist ONLY as `(-32010, format!(...))` tuple arms.
    // A scan that knows just the `code: -32602` form loses all five and the
    // count silently drops to eight.
    const codes = data.errorCodes.value.map((e: { code: number }) => e.code);
    for (const c of [-32010, -32011, -32012, -32013, -32014]) expect(codes).toContain(c);
  });

  it("fails when the source emits a code that is neither documented nor excepted", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/node/src/rpc.rs") return text;
      const out = text.replace(
        'code: -32700,',
        'code: -32777,\n        message: "invented".to_string(),\n    });\n    let _unused = RpcError {\n        code: -32700,'
      );
      landed = out.includes("-32777");
      return out;
    });
    expect(landed, "the doctored code was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("-32777");
  });

  it("fails when a code appears in a form the structured scan does not know", () => {
    // The broad sweep cannot miss a code; the structured patterns can. Making
    // the two disagree is what proves the structured scan is being checked.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/node/src/rpc.rs") return text;
      const out = text.replace("fn handle_get_nonce(", "fn unreachable_probe() -> i64 { -32999 }\n\nfn handle_get_nonce(");
      landed = out.includes("-32999");
      return out;
    });
    expect(landed, "the doctored emission form was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("undercounting");
  });
});

describe("generate-console-data: limits cross-checked against their constants", () => {
  it("reads all six limits from source with a file and line", () => {
    const names = data.sourceLimits.value.map((l: { name: string }) => l.name);
    expect(names).toContain("MAX_RPC_REQUESTS_PER_SEC");
    expect(names).toContain("MAX_TX_SIZE");
    expect(data.sourceLimits.value).toHaveLength(6);
    for (const l of data.sourceLimits.value) expect(l.line).toBeGreaterThan(0);
  });

  it("fails when a constant and the document's Limits table disagree", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/node/src/rpc.rs") return text;
      const out = text.replace(
        "const MAX_RPC_REQUESTS_PER_SEC: usize = 100;",
        "const MAX_RPC_REQUESTS_PER_SEC: usize = 250;"
      );
      landed = out.includes("= 250;");
      return out;
    });
    expect(landed, "the doctored limit was not applied").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("MAX_RPC_REQUESTS_PER_SEC");
  });

  it("fails rather than passing vacuously when the table stops naming constants", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace("`MAX_RPC_REQUESTS_PER_SEC`", "the rate limiter");
      landed = !out.includes("`MAX_RPC_REQUESTS_PER_SEC`");
      return out;
    });
    expect(landed, "the constant reference was not removed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("no longer names every constant");
  });
});

describe("generate-console-data: fees for every transaction type", () => {
  it("covers all eleven types, including the two the cookbook omits", () => {
    expect(data.fees.value).toHaveLength(11);
    const byName = Object.fromEntries(
      data.fees.value.map((f: { name: string; minFee: number }) => [f.name, f.minFee])
    );
    expect(byName.transfer).toBe(100);
    expect(byName.registerAiEntityWithKey).toBe(5000);
    expect(byName.entityUpgrade).toBe(5000);
  });

  it("fails when a transaction type has no fee arm", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/execution/src/lib.rs") return text;
      const out = text.replace(
        "ENTITY_UPGRADE_PAYLOAD_V1 => Ok(MIN_FEE_ENTITY_UPGRADE),",
        ""
      );
      landed = !out.includes("ENTITY_UPGRADE_PAYLOAD_V1 => Ok(MIN_FEE_ENTITY_UPGRADE),");
      return out;
    });
    expect(landed, "the fee arm was not removed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("ENTITY_UPGRADE_PAYLOAD_V1");
  });
});

describe("generate-console-data: the transaction wire layout", () => {
  it("walks the full envelope, and its widths add up to the overhead constant", () => {
    const w = data.txWireLayout.value;
    expect(w.fields.map((f: { field: string }) => f.field)).toEqual([
      "version", "from", "pubkey", "nonce", "fee", "payload_len", "payload",
    ]);
    const fixed = w.fields.reduce((a: number, f: { bytes: number | null }) => a + (f.bytes ?? 0), 0);
    expect(fixed + w.signatureBytes).toBe(w.overhead);
  });

  it("records that the envelope is little-endian", () => {
    const w = data.txWireLayout.value;
    const nonce = w.fields.find((f: { field: string }) => f.field === "nonce");
    expect(nonce.endianness).toBe("little");
  });

  it("fails when a field is dropped, rather than publishing wrong offsets", () => {
    // This is the bug the arithmetic check exists for: the payload-length
    // argument contains a paren, and a parser that stops at the first one
    // drops the field and shifts every offset after it.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/codec/src/lib.rs") return text;
      const out = text.replace("    write_u64_le(&mut out, tx.fee);\n", "");
      landed = !out.includes("write_u64_le(&mut out, tx.fee);");
      return out;
    });
    expect(landed, "the wire field was not removed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("TX_V1_OVERHEAD");
  });
});

describe("generate-console-data: the quorum rule", () => {
  it("reads one expression agreed by both implementing sites", () => {
    expect(data.quorum.value.sites).toHaveLength(2);
    expect(data.quorum.value.expression).toContain("n - 1");
    for (const s of data.quorum.value.sites) {
      expect(s.expression).toBe(data.quorum.value.expression);
    }
  });

  it("fails when the two sites stop agreeing", () => {
    // leader.rs carries the formula TWICE: once in a comment on the line above
    // and once as the expression. A plain string replace hits the comment,
    // which scanRust strips, so the probe lands somewhere the gate cannot see
    // and the test passes for the wrong reason. Target the code line, and
    // assert the doctoring landed in scanned scope rather than merely landing.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/consensus_types/src/leader.rs") return text;
      const out = text.replace(
        /^(\s*)2 \* \(\(n - 1\) \/ 3\) \+ 1(\s*)$/m,
        "$12 * ((n - 1) / 4) + 1$2"
      );
      const codeLines = out
        .split("\n")
        .filter((l) => !l.trim().startsWith("//"))
        .join("\n");
      landed = codeLines.includes("2 * ((n - 1) / 4) + 1");
      return out;
    });
    expect(landed, "the quorum expression was not altered outside a comment").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("quorum rule differs");
  });
});

describe("generate-console-data: retention horizons", () => {
  it("publishes both horizons in blocks, never in wall-clock time", () => {
    const r = data.retentionHorizons.value;
    expect(r.disk.value).toBe(50000);
    expect(r.index.value).toBe(100000);
    expect(JSON.stringify(r)).not.toMatch(/hour|minute|second/i);
  });

  it("fails when the index no longer reaches back at least as far as the disk", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/node/src/main.rs") return text;
      const out = text.replace("const MAX_INDEX_ENTRIES: usize = 100_000;", "const MAX_INDEX_ENTRIES: usize = 1_000;");
      landed = out.includes("MAX_INDEX_ENTRIES: usize = 1_000;");
      return out;
    });
    expect(landed, "the index horizon was not altered").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("inverted");
  });
});

describe("generate-console-data: SDK coverage", () => {
  it("matches builders by the discriminant they emit, not by their name", () => {
    // The three SDKs name the same builder three different ways, so a
    // name-matched join reports Python as missing three builders it has.
    const c = data.sdkCoverage.value;
    expect(c.totals).toEqual({
      txTypes: 11,
      rustBuilders: 11,
      typescriptBuilders: 10,
      pythonBuilders: 11,
    });
    const upgrade = c.builders.find((b: { txType: string }) => b.txType === "entityUpgrade");
    expect(upgrade.python).toBe("build_entity_upgrade_payload");
    expect(upgrade.typescript).toBeNull();
  });

  it("states the TypeScript type gap in numbers", () => {
    const c = data.sdkCoverage.value;
    expect(c.signalTypes.chain).toBe(23);
    expect(c.signalTypes.typescript).toBe(7);
    expect(c.memoryObjectTypes.chain).toBe(16);
    expect(c.memoryObjectTypes.typescript).toBe(5);
  });

  it("treats the Rust SDK's type coverage as structural, and asserts the re-export", () => {
    const c = data.sdkCoverage.value;
    expect(c.signalTypes.rust).toBe(c.signalTypes.chain);
    expect(c.signalTypes.rustIsStructural).toBe(true);
  });

  it("fails when the Rust SDK stops re-exporting the chain's own enums", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "sdk/novai-sdk/src/lib.rs") return text;
      const out = text.replace("AiSignalType,", "");
      landed = !out.includes("AiSignalType,");
      return out;
    });
    expect(landed, "the re-export was not removed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("structural");
  });

  it("records that the Rust SDK cannot be consumed from a registry", () => {
    const w = data.sdkCoverage.value.workspaceCoupling.rust;
    expect(w.consumableFromRegistry).toBe(false);
    expect(w.pathDependencies.length).toBeGreaterThan(0);
  });
});

describe("generate-console-data: the remaining source datasets", () => {
  it("reads the seven capability bits", () => {
    expect(data.capabilityBits.value).toHaveLength(7);
    const bit5 = data.capabilityBits.value.find((c: { bit: number }) => c.bit === 5);
    expect(bit5.capability).toBe("submit_reputation_updates");
    expect(bit5.hex).toBe("0x20");
  });

  it("reads the signal payload base length and its tails", () => {
    expect(data.signalPayloads.value.baseLength.value).toBe(66);
    const byName = Object.fromEntries(
      data.signalPayloads.value.tails.map((t: { name: string; value: number }) => [t.name, t.value])
    );
    expect(byName.REPUTATION_UPDATE_EXTRA_LEN).toBe(35);
    expect(byName.SIGNAL_PURCHASE_EXTRA_LEN).toBe(41);
    expect(byName.STAKE_DEPOSIT_EXTRA_LEN).toBe(16);
    expect(byName.STAKE_SLASH_EXTRA_LEN).toBe(51);
  });

  it("reads the percentage fees against their shared denominator", () => {
    const b = data.bpsFees.value;
    expect(b.denominator).toBe(10000);
    const market = b.entries.find((e: { constant: string }) => e.constant === "MARKETPLACE_FEE_BPS");
    expect(market.bps).toBe(200);
    expect(market.percent).toBe(2);
  });

  it("carries the document's own Observed gaps table", () => {
    expect(data.observedGaps.value.length).toBeGreaterThan(3);
    const joined = JSON.stringify(data.observedGaps.value);
    expect(joined).toContain("mempool");
  });

  it("gives every method a source link into the dispatch table", () => {
    expect(data.sourceRefs.value).toHaveLength(29);
    for (const r of data.sourceRefs.value) {
      expect(r.file).toBe("crates/node/src/rpc.rs");
      expect(r.line).toBeGreaterThan(0);
    }
  });

  it("fails when a constant is declared twice, rather than reading whichever came first", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/types/src/lib.rs") return text;
      const out = text.replace(
        "pub const MAX_TX_SIZE: usize = 128 * 1024;",
        "pub const MAX_TX_SIZE: usize = 128 * 1024;\npub const MAX_TX_SIZE: usize = 64 * 1024;"
      );
      landed = (out.match(/pub const MAX_TX_SIZE/g) ?? []).length === 2;
      return out;
    });
    expect(landed, "the duplicate constant was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("ambiguous");
  });
});

// ---------------------------------------------------------------------------
// The table parser, and the two independent gates on the truncation defect.
//
// Every test here injects a violation and asserts the probe landed before
// asserting the gate fired. Three separate scans in the previous gate reported
// clean because they matched nothing, and each was caught only by feeding it
// something it had to catch.
// ---------------------------------------------------------------------------

const ENTITY_ID_ROW =
  "| `entity_id` | `hex32` | the canonical entity id, `blake3(\"NOVAI_AI_ENTITY_ID_V1\" \\|\\| code_hash \\|\\| creator)` |";

// Built from code points, never written literally. This file is scanned by the
// dash gate, so a test that quotes the reference's minus sign would fail the
// gate it exists to exercise.
const U_MINUS = String.fromCodePoint(0x2212);
const U_LTE = String.fromCodePoint(0x2264);
const RANGE_CLAUSE = `\`end_height ${U_MINUS} start_height > 10000\` (range queries)`;

describe("generate-console-data: the row splitter", () => {
  it("publishes the whole entity id derivation, escaped pipes and all", () => {
    const m = data.methods.value.find((x: { name: string }) => x.name === "novai_getAiEntity");
    const note = m.params.list.find((p: { field: string }) => p.field === "entity_id").notes;
    // The defect published this cut at the first escaped pipe, ending on a
    // dangling backslash. entity_id is the required parameter of fourteen of
    // the twenty-nine methods, so the page gave no way to compute one.
    expect(note).toContain("code_hash || creator");
    expect(note).not.toMatch(/\\$/);
    expect(JSON.stringify(data).split("code_hash || creator").length - 1).toBeGreaterThan(0);
  });

  it("fails on a row that splits into more cells than its header declares", () => {
    // Unescape the pipes on line 400, which recreates the original defect
    // exactly rather than inventing a new one: seven cells against a header
    // of three.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(ENTITY_ID_ROW, ENTITY_ID_ROW.replace(/\\\|/g, "|"));
      landed = out !== text && out.includes('"NOVAI_AI_ENTITY_ID_V1" || code_hash');
      return out;
    });
    expect(landed, "the escaped pipes were not unescaped").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("cells and its header declares");
  });

  it("fails on a cell ending in a dangling backslash", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace("| `id` | `hex32` | the VK registry handle |", "| `id` | `hex32` | the VK registry handle \\ |");
      landed = out.includes("registry handle \\ |");
      return out;
    });
    expect(landed, "the trailing backslash was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("dangling operator or backslash");
  });

  it("fails on a cell carrying an unbalanced backtick", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace("| `id` | `hex32` | the VK registry handle |", "| `id` | `hex32` | the VK `registry handle |");
      landed = out.includes("the VK `registry handle |");
      return out;
    });
    expect(landed, "the unbalanced backtick was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("unbalanced backtick");
  });
});

describe("generate-console-data: category-common error scoping", () => {
  it("does not publish the range error on the method that takes no range", () => {
    const byName = new Map(data.methods.value.map((m: { name: string }) => [m.name, m]));
    const height = byName.get("novai_getSignalsByHeight") as { params: { list: { field: string }[] }; errors: { list: { when: string }[]; scopedOut: unknown[] } };
    // Its only parameter is height, and its handler holds no range check.
    expect(height.params.list.map((p) => p.field)).toEqual(["height"]);
    expect(height.errors.list.some((e) => /start_height/.test(e.when))).toBe(false);
    expect(height.errors.scopedOut).toHaveLength(1);
    // And the row stays where it is true.
    for (const name of ["novai_getSignalsByIssuer", "novai_getSignalsByType"]) {
      const m = byName.get(name) as { errors: { list: { when: string }[] } };
      expect(m.errors.list.some((e) => /start_height/.test(e.when)), `${name} lost a row it should keep`).toBe(true);
    }
  });

  it("fails rather than silently scoping nothing, when no common row names a field at all", () => {
    // Strip the backticks so the clause carries no field identifier. Every
    // common row is then inherited by every method in the category, nothing is
    // scoped out, and the vacuity guard must fire rather than let the filter
    // report a clean run by matching nothing. That is exactly how the original
    // defect survived two adversarial passes.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(RANGE_CLAUSE, `end_height ${U_MINUS} start_height > 10000 (range queries)`);
      landed = out !== text && !out.includes(RANGE_CLAUSE);
      return out;
    });
    expect(landed, "the range clause backticks were not stripped").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("category-common scoping matched no rows");
  });
});

describe("generate-console-data: error clauses are quoted verbatim", () => {
  it("publishes the reference's own minus sign rather than an ASCII substitute", () => {
    // Asserted against the FILE, not against a re-stringified object: the file
    // is what ships, and it is what has to stay ASCII. JSON.stringify of the
    // parsed object would emit the literal code point and prove nothing about
    // either property.
    const raw = readFileSync(DATA, "utf8");
    expect(raw).toContain("\\u2212");
    expect(raw).not.toContain("end_height - start_height");
    // And the value a reader receives is the character the reference writes.
    const byName = new Map(data.methods.value.map((m: { name: string }) => [m.name, m]));
    const issuer = byName.get("novai_getSignalsByIssuer") as { errors: { list: { when: string }[] } };
    expect(issuer.errors.list.some((e) => e.when.includes(U_MINUS))).toBe(true);
  });

  it("fails when a normalised markdown code span reaches the page", () => {
    // The gate filtered to .rs sources only, so a doc code span could be
    // rewritten and published with nothing reporting it. This injection also
    // exercises the needle escaping: the span carries U+2264, and before the
    // fix the check searched the ASCII-escaped payload for a needle still
    // holding a literal one, which could never match.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(`\`end - start ${U_LTE} 10000\``, `\`end ${U_MINUS} start ${U_LTE} 10000\``);
      landed = out !== text && out.includes(`\`end ${U_MINUS} start`);
      return out;
    });
    expect(landed, "the minus sign was not injected into a params note").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("published in rewritten form");
  });
});

describe("generate-console-data: the inherited-meaning gate, proven by injection", () => {
  it("refuses an inherited clause naming a field the inheriting method does not declare", () => {
    // novai_listVkRegistrations aliases its Errors block onto
    // novai_getVkRegistration. Adding a row there that names getVkRegistration's
    // own `id` in an expression gives listVkRegistrations a clause about a field
    // it has no parameter for, and the carried exception does not cover this
    // text, so the build must stop.
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(
        "| `-32602` | `id` isn't 32 bytes |",
        "| `-32602` | `id` isn't 32 bytes |\n| `-32602` | `id` decoded to the wrong length |"
      );
      landed = out.includes("`id` decoded to the wrong length");
      return out;
    });
    expect(landed, "the extra error row was not inserted").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("an alias inherited a meaning that is false");
  });
});

describe("generate-console-data: the curl-agrees-with-params gate, proven by injection", () => {
  it("refuses a curl passing a field the params table does not declare", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace('"novai_getSignalsByHeight","params":{"height":453}', '"novai_getSignalsByHeight","params":{"heightx":453}');
      landed = out.includes('{"heightx":453}');
      return out;
    });
    expect(landed, "the curl parameter was not renamed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("which the params table does not declare");
  });
});

describe("generate-console-data: the two new document defects", () => {
  it("carries both, each measured on both sides", () => {
    const ids = data.drift.value.knownExceptions.map((e: { id: string }) => e.id);
    expect(ids).toContain("latestblock-claims-only-global-errors");
    expect(ids).toContain("sla-seller-cap-does-not-exist");
  });

  it("fails, naming the entry to delete, when the seller sentence is corrected", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(" Bounded internally by the per-buyer cap (= 8 in v1).", "");
      landed = !out.includes("Bounded internally by the per-buyer cap");
      return out;
    });
    expect(landed, "the seller cap sentence was not removed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("sla-seller-cap-does-not-exist");
  });

  it("fails, naming the entry to delete, when getLatestBlock's error block is corrected", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(
        "**Errors**: only the global ones (`-32600` malformed envelope, `-32601` unknown method).",
        "**Errors**: `-32002` for a block that fails to load or hash."
      );
      landed = !out.includes("**Errors**: only the global ones");
      return out;
    });
    expect(landed, "the getLatestBlock errors block was not rewritten").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("latestblock-claims-only-global-errors");
  });
});

describe("generate-console-data: the getNonce exception", () => {
  it("is carried while the document still calls the two interchangeable", () => {
    const ids = data.drift.value.knownExceptions.map((e: { id: string }) => e.id);
    expect(ids).toContain("getnonce-documented-as-interchangeable");
  });

  it("fails, naming the entry to delete, when the document is fixed", () => {
    let landed = false;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "docs/RPC_REFERENCE.md") return text;
      const out = text.replace(
        "Cheaper than `getBalance` if you don't need the balance.",
        "Answers from the mempool admission cursor, which is not the committed account nonce."
      );
      landed = !out.includes("Cheaper than `getBalance`");
      return out;
    });
    expect(landed, "the getNonce wording was not changed").toBe(true);
    const res = generate(root);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("getnonce-documented-as-interchangeable");
  });
});

describe("generate-console-data: the source-link gate, proven by moving an arm", () => {
  // The in-generator assertion reads the recorded line back from the SAME parse
  // that produced it, so it cannot detect staleness and is only a parser
  // self-check. What actually stops a published line number from rotting is
  // --check in prebuild. These two tests prove that, by moving the dispatch
  // block in a fixture rather than by observing the gate pass against an
  // unmodified tree.
  const ANCHOR = "let http_response = match rpc_request.method.as_str() {";
  const SHIFT = 3;

  function shiftedRoot(): string {
    // The probe must prove the block actually MOVED by the expected amount,
    // not merely that the file changed. "out !== text" would be satisfied by
    // any edit at all, including one the gate cannot see.
    let movedBy = -1;
    const root = fixtureRoot((rel, text) => {
      if (rel !== "crates/node/src/rpc.rs") return text;
      const lineOfAnchor = (s: string) => s.split("\n").findIndex((l) => l.includes(ANCHOR));
      const wasAt = lineOfAnchor(text);
      const out = text.replace(ANCHOR, "\n".repeat(SHIFT) + ANCHOR);
      movedBy = lineOfAnchor(out) - wasAt;
      return out;
    });
    expect(movedBy, `expected the dispatch block to move down ${SHIFT} lines`).toBe(SHIFT);
    return root;
  }

  it("every recorded line moves by exactly the shift when the dispatch block moves", () => {
    const root = shiftedRoot();
    const out = join(root, "out.json");
    const res = run(SCRIPT, ["--root", root, "--out", out, "--openrpc", join(root, "openrpc.json")]);
    expect(res.status, res.output).toBe(0);
    const moved = JSON.parse(readFileSync(out, "utf8"));
    rmSync(root, { recursive: true, force: true });

    const before = new Map<string, number>(
      data.sourceRefs.value.map((r: { name: string; line: number }) => [r.name, r.line])
    );
    expect(moved.sourceRefs.value).toHaveLength(29);
    for (const r of moved.sourceRefs.value) {
      expect(r.line, `${r.name} did not track the moved dispatch block`).toBe(
        (before.get(r.name) as number) + SHIFT
      );
    }
  });

  it("--check fails against the committed data once an arm has moved", () => {
    // This is the gate: prebuild runs --check, so a moved arm cannot ship with
    // links still pointing at the old lines.
    const root = shiftedRoot();
    const res = run(SCRIPT, ["--check", "--root", root, "--out", DATA, "--openrpc", OPENRPC]);
    rmSync(root, { recursive: true, force: true });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("stale");
  });
});
