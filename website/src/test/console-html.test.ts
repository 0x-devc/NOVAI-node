import { describe, it, expect } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";

const SCRIPT = resolve("scripts/generate-console-html.mjs");
const DATA = resolve("src/data/console-data.generated.json");

/** Every console page on disk, keyed by its path relative to the web root. */
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
const GENERATED_FILES = ["console/all.html", "console/names.html"];

const pages = new Map<string, string>(
  [...PAGE_FILES, ...GENERATED_FILES].map((f) => [f, readFileSync(resolve(f), "utf8")])
);

/** The whole console as one string, for "does this appear anywhere" checks. */
const committed = [...pages.values()].join("\n");
/**
 * The eight canonical pages only.
 *
 * all.html is a deliberate second copy of every section, so anything COUNTED
 * has to be counted here or every total doubles. Anything merely looked up can
 * use `committed`.
 */
const canonical = PAGE_FILES.map((f) => pages.get(f) as string).join("\n");
/** The reference page, where the index, the methods and the record shapes live. */
const rpc = pages.get("console/rpc.html") as string;
const data = JSON.parse(readFileSync(DATA, "utf8"));

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
 * A doctored copy of the whole page set, checked in isolation.
 *
 * The gates run across eight files now, so a single-file harness would only
 * ever exercise one of them. The whole set is copied to a temp root, one page
 * is edited, and --check runs against the copy: the working tree is never
 * touched, and the global gates see a realistic page set.
 */
function withPages(file: string, edit: (t: string) => string): { status: number; output: string } {
  const dir = mkdtempSync(join(tmpdir(), "console-pages-"));
  for (const [rel, text] of pages) {
    const target = join(dir, rel);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, rel === file ? edit(text) : text);
  }
  if (file !== "*") {
    const before = pages.get(file) as string;
    expect(readFileSync(join(dir, file), "utf8"), "the doctoring produced an identical file").not.toBe(before);
  }
  const res = run(["--check", "--web-root", dir, "--data", DATA]);
  rmSync(dir, { recursive: true, force: true });
  return res;
}

/** Backwards-compatible alias for the tests that doctor the landing page. */
const withHtml = (edit: (t: string) => string) => withPages("console.html", edit);

describe("generate-console-html: the marker machinery", () => {
  it("is green against the committed page", () => {
    const res = run(["--check"]);
    expect(res.output).toContain("check ok");
    expect(res.status).toBe(0);
  });

  it("fails when a generated region is hand-edited", () => {
    // The failure this exists for: an edit inside a region is disposable, and
    // the next write would erase it silently.
    const res = withHtml((t) => {
      const marker = "@@console-generated:connect@@ -->";
      const i = t.indexOf(marker) + marker.length;
      return t.slice(0, i) + "\n          <p>hand edit</p>" + t.slice(i);
    });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("hand-edited");
  });

  it("fails when an opening marker has no closing pair", () => {
    const res = withPages("console/sdks.html", (t) => t.replace("<!-- @@console-generated:/sdks@@ -->\n", ""));
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("marker pairing");
  });

  it("fails when a closing marker has no opening pair", () => {
    const res = withPages("console/sdks.html", (t) => t.replace("<!-- @@console-generated:sdks@@ -->\n", ""));
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("marker pairing");
  });

  it("fails when the page carries fewer regions than the script writes", () => {
    const res = withPages("console/network.html", (t) => {
      const open = "<!-- @@console-generated:gaps@@ -->";
      const close = "<!-- @@console-generated:/gaps@@ -->";
      const a = t.indexOf(open);
      const b = t.indexOf(close) + close.length;
      return t.slice(0, a) + t.slice(b);
    });
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("region count");
    expect(res.output).toContain("gaps");
  });

  it("fails when a region id appears twice", () => {
    const res = withPages("console/verify.html", (t) =>
      t.replace(
        "<!-- @@console-generated:verify@@ -->",
        "<!-- @@console-generated:verify@@ -->\n          <!-- @@console-generated:/verify@@ -->\n          <!-- @@console-generated:verify@@ -->"
      )
    );
    expect(res.status).not.toBe(0);
    expect(res.output).toMatch(/region count|duplicate/);
  });

  it("fails when regions are nested rather than sequential", () => {
    const res = withPages("console/verify.html", (t) =>
      t.replace(
        "<!-- @@console-generated:/verify@@ -->",
        "<!-- @@console-generated:verify@@ -->\n          <!-- @@console-generated:/verify@@ -->"
      )
    );
    expect(res.status).not.toBe(0);
    expect(res.output).toMatch(/marker (pairing|nesting)/);
  });

  it("does not mistake a near-miss comment for a marker", () => {
    // The sigil is what makes a marker unforgeable by ordinary editing. A
    // comment that reads like one but lacks the @@ delimiters must be inert,
    // or a stray comment could silently capture a region.
    const res = withPages("console/verify.html", (t) =>
      t.replace(
        "<!-- @@console-generated:verify@@ -->",
        "<!-- console-generated:verify -->\n          <!-- @@console-generated:verify@@ -->"
      )
    );
    // The near-miss is ignored, so the page is still valid and only differs by
    // an inert comment; the render is unchanged and --check passes on content.
    expect(res.output).not.toContain("region count");
  });
});

describe("generate-console-html: what actually reached the page", () => {
  it("never renders markup as visible text", () => {
    // The failure this catches: built HTML passed to a helper that escapes,
    // so the reader sees a literal <a class=...> in the middle of a sentence.
    // It also forced the document to overflow at 360px, because an unbreakable
    // run of escaped markup cannot wrap.
    for (const leaked of ["&lt;a class", "&lt;code", "&lt;p class", "&lt;span class", "&lt;strong&gt;"]) {
      expect(committed, `escaped markup rendered as text: ${leaked}`).not.toContain(leaked);
    }
  });

  it("wraps every table in a scroll container so the document never overflows", () => {
    const tables = (committed.match(/<table/g) ?? []).length;
    const wrappers = (committed.match(/class="console-scroll"/g) ?? []).length;
    expect(tables).toBeGreaterThan(5);
    expect(wrappers).toBe(tables);
  });

  it("carries no placeholder, filler or coming-soon copy", () => {
    for (const banned of ["layout filler", "not real data", "coming soon", "lorem", "TODO", "0,000,000"]) {
      expect(committed.toLowerCase(), `console.html still contains ${banned}`).not.toContain(banned.toLowerCase());
    }
  });

  it("no longer claims the reference cannot drift", () => {
    // Check rendered prose, not the raw file: an HTML comment above the
    // paragraph quotes the old claim to explain why it went, and the SDK
    // section makes a different and true claim about the Rust SDK's type
    // coverage being structural. Neither is a promise made to the reader.
    const visible = committed.replace(/<!--[\s\S]*?-->/g, "");
    const referenceClaim = visible.slice(0, visible.indexOf('id="sdks"'));
    expect(referenceClaim).not.toContain("cannot drift");
    expect(visible).toContain("cross-checked across four independent sources");
    // And it says what the check actually covers. The earlier wording implied
    // the four-way gate compared the whole reference; it compares method names.
    expect(visible).toContain("That check is over names");
    // The count is derived now, not typed. It was hardcoded here and in
    // console.html, fifteen lines above a generated tile reading the same
    // number, and the two stopped agreeing the moment a sixth was carried.
    expect(visible).toContain(`There are ${data.drift.value.knownExceptions.length} of those today`);
  });

  it("renders every method with an anchor and a source link", () => {
    for (const m of data.methods.value) {
      expect(committed, `${m.name} has no anchor`).toContain(`id="${m.name.toLowerCase()}"`);
    }
    const links = committed.match(/blob\/main\/crates\/node\/src\/rpc\.rs#L\d+/g) ?? [];
    expect(links.length).toBeGreaterThanOrEqual(data.methods.value.length);
  });

  it("renders all thirteen error codes including the undocumented one", () => {
    for (const e of data.errorCodes.value) {
      expect(committed, `code ${e.code} missing`).toContain(`<code>${e.code}</code>`);
    }
    expect(committed).toContain("not in the reference");
  });

  it("renders the eleven transaction types with their fees", () => {
    for (const f of data.fees.value) {
      expect(committed, `${f.name} missing`).toContain(`<code>${f.name}</code>`);
    }
  });

  it("states the retention window in blocks and never in wall-clock time", () => {
    expect(committed).toContain("50,000");
    const retention = committed.slice(committed.indexOf('id="parameters"'), committed.indexOf('id="gaps"'));
    expect(retention).not.toMatch(/\b\d+(\.\d+)?\s*(hours?|minutes?)\b/i);
  });

  it("keeps the reference spine expanded, and collapses only the lookup tables", () => {
    // Content inside a closed <details> is unreachable by find-in-page in
    // Firefox and Safari, so the 29 method entries must not be collapsed.
    const rpcSection = committed.slice(committed.indexOf('id="rpc"'), committed.indexOf('id="errors"'));
    expect(rpcSection).not.toContain("<details");
    expect(committed).toContain("<details");
  });

  it("publishes the SDK asymmetry rather than implying parity", () => {
    expect(committed).toContain("pip install novai-sdk");
    expect(committed).toContain("not on crates.io");
    expect(committed).toContain("not published to npm");
  });

  it("attaches every caveat from a per-method declaration and never from prose", () => {
    // The mechanism this replaced regex-scanned each exception's summary and
    // why for method names and labelled every method it found mentioned. It
    // failed in BOTH directions on one page: novai_getBalance was given
    // novai_getNonce's caveat, which is the reverse of the truth for
    // getBalance, and novai_submitTransaction was given none at all.
    const index = committed.slice(committed.indexOf('id="rpc"'), committed.indexOf("console-method"));
    const rowFor = (name: string) => {
      const rows = index.match(/<tr>[\s\S]*?<\/tr>/g) ?? [];
      const hit = rows.filter((r) => r.includes(`href="#${name.toLowerCase()}"`));
      expect(hit.length, `${name} should appear in exactly one index row`).toBe(1);
      return hit[0];
    };
    const pillsIn = (frag: string) =>
      [...frag.matchAll(/<span class="console-pill">([^<]*)<\/span>/g)].map((m) => m[1]);

    for (const e of data.drift.value.knownExceptions) {
      expect(index, `exception id ${e.id} leaked into the index as a note`).not.toContain(e.id);
    }

    // The regression itself, stated as the thing that must never come back.
    expect(rowFor("novai_getBalance")).not.toContain("mempool cursor");
    expect(rowFor("novai_getBalance")).toContain("nonce here is the committed one");
    expect(rowFor("novai_getNonce")).toContain("mempool cursor, not the account nonce");
    expect(rowFor("novai_submitTransaction")).toContain("emits an undocumented code");

    // Both directions, for every method.
    for (const m of data.methods.value) {
      const declared = new Set<string>(m.caveats.map((c: { label: string }) => c.label));
      if (m.answersNull) declared.add("can answer null");
      if (m.withheld) declared.add("not documented here");
      expect(new Set(pillsIn(rowFor(m.name))), `${m.name} index pills`).toEqual(declared);
    }
  });

  it("has deleted the prose-scanning mechanism, not merely stopped calling it", () => {
    // Every other assertion here can pass on a renderer that ALSO still scans
    // prose. Deleting it has to be checked directly.
    const gen = readFileSync(SCRIPT, "utf8");
    expect(gen).not.toContain("CAVEAT_LABELS");
    expect(gen).not.toMatch(/matchAll\(\/novai_/);
  });

  it("strikes every false claim at the point the reader meets it", () => {
    const struck = (canonical.match(/<del /g) ?? []).length;
    // Corrections land on methods and, for drift that belongs to the whole
    // surface rather than to one handler, on error codes.
    const withWrongText = [
      ...data.methods.value.flatMap((m: { corrections: { wrongText: string | null }[] }) => m.corrections),
      ...data.codeCorrections.value,
    ].filter((c: { wrongText: string | null }) => c.wrongText);
    expect(struck).toBe(withWrongText.length);
    expect(struck).toBeGreaterThan(0);
    // Compare as reading text, not as markup: rich() turns `x` into a <code>
    // element, so a raw-substring check would compare two different languages.
    const asText = committed
      .replace(/<[^>]+>/g, "")
      .replace(/&quot;/g, '"')
      .replace(/&#39;/g, "'")
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&")
      .replace(/\s+/g, " ");
    const stripMd = (t: string) =>
      t.replace(/`([^`]*)`/g, "$1").replace(/\*\*([^*]*)\*\*/g, "$1").replace(/\s+/g, " ").trim();
    for (const m of data.methods.value) {
      for (const c of m.corrections) {
        expect(asText, `${m.name}: correction from ${c.exceptionId} missing`).toContain(stripMd(c.correction));
      }
    }
  });

  it("withholds the faucet's runnable content while keeping it counted", () => {
    const withheld = data.methods.value.filter((m: { withheld: unknown }) => m.withheld);
    expect(withheld).toHaveLength(1);
    expect(withheld[0].name).toBe("novai_faucet");
    expect(data.methods.value).toHaveLength(29);

    const start = committed.indexOf('id="novai_faucet"');
    const end = committed.indexOf("@@console-generated:/rpc-methods@@");
    expect(start).toBeGreaterThan(-1);
    const block = committed.slice(start, end);
    expect(block).not.toContain("curl -s");
    expect(block).not.toContain("<table");
    expect(block).not.toContain("10000000");
    expect(block).toContain("not documented here");
    // The anchor and the source link stay, so a deep link and the route to the
    // handler both survive.
    expect(block).toMatch(/rpc\.rs#L\d+/);
    // And the reference's own brief, whose parenthetical repeats the false
    // gating claim, does not reach the index.
    const index = committed.slice(committed.indexOf('id="rpc"'), committed.indexOf("console-method"));
    expect(index).not.toContain("dev mode only");
  });

  it("labels reference sample responses so a stale height is not read as live", () => {
    // A withheld method's sample response is deliberately not rendered, so it
    // must not be counted here either.
    const withSamples = data.methods.value.filter(
      (m: { sampleResponse: string | null; withheld: unknown }) => m.sampleResponse && !m.withheld
    );
    expect(withSamples.length).toBeGreaterThan(10);
    const labels = (canonical.match(/Example response from the reference/g) ?? []).length;
    expect(labels).toBe(withSamples.length);
  });

  it("carries the five drift exceptions in known gaps", () => {
    for (const e of data.drift.value.knownExceptions) {
      expect(committed, `exception ${e.id} missing`).toContain(e.id);
    }
  });

  it("labels the captured first-call output with when it was taken", () => {
    expect(committed).toContain("Captured 2026-08-29T");
    expect(committed).toContain("yours will differ");
  });
});
