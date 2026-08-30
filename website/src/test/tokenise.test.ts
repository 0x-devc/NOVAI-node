import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { tokeniseSchema, tokeniseShell, assertLossless } from "../../scripts/tokenise.mjs";

const data = JSON.parse(readFileSync(resolve("src/data/console-data.generated.json"), "utf8"));
// The console is eight pages plus two generated find surfaces. Reading only
// the landing page would check seven code blocks out of ninety.
const PAGE_FILES = [
  "console.html",
  "console/rpc.html",
  "console/errors.html",
  "console/transactions.html",
  "console/entities.html",
  "console/sdks.html",
  "console/network.html",
  "console/verify.html",
];
const committed = PAGE_FILES.map((f) => readFileSync(resolve(f), "utf8")).join("\n");

type Tok = { cls: string; text: string };

describe("tokenise: the losslessness invariant", () => {
  it("reproduces every schema fence on the page byte for byte", () => {
    const known = new Set<string>(data.methods.value.flatMap((m: { result?: { recordTypes?: string[] } }) => m.result?.recordTypes ?? []));
    const fences = data.methods.value
      .flatMap((m: { result?: { envelope?: string; recordShape?: string }; sampleResponse?: string }) => [
        m.result?.envelope,
        m.result?.recordShape,
        m.sampleResponse,
      ])
      .filter(Boolean) as string[];
    expect(fences.length).toBeGreaterThan(30);
    for (const f of fences) {
      const toks = tokeniseSchema(f, known);
      expect(() => assertLossless(f, toks)).not.toThrow();
    }
  });

  it("reproduces every curl on the page byte for byte", () => {
    const curls = data.methods.value.map((m: { curl: string }) => m.curl).filter(Boolean);
    expect(curls.length).toBe(29);
    for (const c of curls) {
      const toks = tokeniseShell(c);
      expect(() => assertLossless(c, toks)).not.toThrow();
    }
  });

  it("detects a tokeniser that drops input, so the invariant is not vacuous", () => {
    // The check has to be shown to fail, or "every block is lossless" could
    // mean the assertion never looked.
    const src = '{ "a": 1 }';
    const lying: Tok[] = [{ cls: "punct", text: "{" }];
    expect(() => assertLossless(src, lying)).toThrow(/lost or altered/);
    const unclassified: Tok[] = [{ cls: "", text: src }];
    expect(() => assertLossless(src, unclassified)).toThrow(/unclassified/);
  });
});

describe("tokenise: what colour is doing", () => {
  it("separates a key from a value and marks a placeholder", () => {
    const toks: Tok[] = tokeniseSchema('{\n  "height": <u64>,\n  "hash": "<hex32>"\n}');
    const cls = (t: string) => toks.filter((x) => x.cls === t).map((x) => x.text);
    expect(cls("prop")).toEqual(['"height"', '"hash"']);
    expect(cls("ph")).toEqual(["<u64>", '"<hex32>"']);
    expect(cls("num")).toEqual([]);
  });

  it("marks a record reference as a type, so it can be linked", () => {
    const toks: Tok[] = tokeniseSchema('{ "channels": [PaymentChannel, ...] }', new Set(["PaymentChannel"]));
    expect(toks.find((t) => t.cls === "type")?.text).toBe("PaymentChannel");
  });

  it("does not mistake a type name inside a string or a comment for a reference", () => {
    const known = new Set(["PaymentChannel"]);
    const inString: Tok[] = tokeniseSchema('{ "kind": "PaymentChannel" }', known);
    expect(inString.some((t) => t.cls === "type")).toBe(false);
    const inComment: Tok[] = tokeniseSchema('{ // a PaymentChannel\n}', known);
    expect(inComment.some((t) => t.cls === "type")).toBe(false);
  });

  it("splits a curl into command, flags, variable and argument", () => {
    const toks: Tok[] = tokeniseShell(`curl -s -X POST $URL -H 'Content-Type: application/json'`);
    expect(toks.find((t) => t.cls === "cmd")?.text).toBe("curl");
    expect(toks.filter((t) => t.cls === "flag").map((t) => t.text)).toEqual(["-s", "-X", "-H"]);
    expect(toks.find((t) => t.cls === "var")?.text).toBe("$URL");
  });
});

describe("tokenise: what reached the page", () => {
  it("emits every token class on the rendered page", () => {
    for (const cls of ["prop", "str", "ph", "num", "comment", "punct", "type", "cmd", "flag", "var"]) {
      expect(committed, `no tok-${cls} on the page`).toContain(`class="tok-${cls}"`);
    }
  });

  it("renders no code block as undifferentiated text", () => {
    // The failure this catches: a call site that forgets its lang and falls
    // back to one plain span, which looks fine in a diff and dead on the page.
    const blocks = committed.match(/<pre class="console-pre[^"]*"[^>]*>[\s\S]*?<\/pre>/g) ?? [];
    expect(blocks.length).toBeGreaterThan(80);
    const flat = blocks.filter((b) => !b.includes('class="tok-') && !b.includes('data-lang="none"'));
    expect(flat, `${flat.length} code block(s) carry no tokens`).toHaveLength(0);
  });
});
