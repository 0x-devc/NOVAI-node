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

// ---------------------------------------------------------------------------
// The gates added when the fourteenth defect was found.
//
// Each injects a violation and asserts the probe landed before asserting the
// gate fired. The reason this discipline is not optional here: the prose gate
// already existed, computed the value it needed for the reverse check, and
// threw it away with `void declared;`, so it reported clean for the whole life
// of the page split while the console's opening sentence was missing.
// ---------------------------------------------------------------------------

/** Doctor the GENERATOR itself, and run it against the committed pages. */
function withScript(edit: (t: string) => string): { status: number; output: string; landed: boolean } {
  const dir = mkdtempSync(join(tmpdir(), "console-script-"));
  const src = readFileSync(SCRIPT, "utf8");
  const out = edit(src);
  const copy = join(dir, "generate-console-html.mjs");
  // The generator imports ./tokenise.mjs by relative path, so the sibling has
  // to travel with it or the copy fails to load for the wrong reason.
  writeFileSync(copy, out);
  writeFileSync(join(dir, "tokenise.mjs"), readFileSync(resolve("scripts/tokenise.mjs"), "utf8"));
  let res: { status: number; output: string };
  try {
    const r = execFileSync(process.execPath, [copy, "--check", "--web-root", resolve("."), "--data", DATA], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    res = { status: 0, output: r };
  } catch (err) {
    const e = err as { status: number | null; stdout?: string; stderr?: string };
    res = { status: e.status ?? -1, output: `${e.stdout ?? ""}${e.stderr ?? ""}` };
  }
  rmSync(dir, { recursive: true, force: true });
  return { ...res, landed: out !== src };
}

describe("generate-console-html: the prose gate, both directions", () => {
  it("renders the console's opening sentence rather than an empty paragraph", () => {
    // The defect: renderConnect asked for PROSE.connect, which did not exist,
    // and rich(undefined) returns "". The landing page shipped an empty lead.
    expect(pages.get("console.html")).not.toContain('<p class="console-lead"></p>');
    expect(pages.get("console/all.html")).not.toContain('<p class="console-lead"></p>');
    expect(pages.get("console.html")).toContain("One HTTPS endpoint, no key and no signup");
  });

  it("fails when a rendered PROSE key was never declared", () => {
    const res = withScript((t) => t.replace("${lead(PROSE.connect)}", "${lead(PROSE.connectX)}"));
    expect(res.landed, "the PROSE reference was not renamed").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("rendered and never declared");
  });

  it("fails when a declared PROSE key is never rendered", () => {
    const res = withScript((t) => t.replace("const PROSE = {", "const PROSE = {\n  orphanKey: \"nothing renders this\","));
    expect(res.landed, "the orphan key was not inserted").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("declared and never rendered");
  });

  it("fails when a hand-written sentence states a number the page derives", () => {
    // The exact defect: "Five discrepancies are known" against a derived count.
    // Injected into a key that IS rendered, so the unused-key branch cannot
    // fire first and pass the test for the wrong reason.
    const res = withScript((t) =>
      t.replace(
        '"Requests are rate limited per source IP.',
        '"There are 11 carried exceptions. Requests are rate limited per source IP.'
      )
    );
    expect(res.landed, "the derived number was not injected into PROSE").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("states a derived number");
  });
});

describe("generate-console-html: the cross-reference gate", () => {
  it("resolves every same-page fragment on the page that emits it", () => {
    const res = run(["--check"]);
    expect(res.status, res.output).toBe(0);
  });

  it("fails when a page links to a fragment it does not define", () => {
    // Inserted OUTSIDE any generated region, so --check does not reject it as a
    // hand-edited region first and pass this test for the wrong reason.
    const res = withPages("console/rpc.html", (t) =>
      t.replace(
        '<main id="main" tabindex="-1" class="min-w-0 px-4 py-8 sm:px-6">',
        '<main id="main" tabindex="-1" class="min-w-0 px-4 py-8 sm:px-6"><a href="#does-not-exist">x</a>'
      )
    );
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("does-not-exist");
  });

  it("fails when a cross-page link names a fragment the target page lacks", () => {
    const res = withScript((t) => t.replace('fragment: "signal-types"', 'fragment: "signal-types-gone"'));
    expect(res.landed, "the fragment was not renamed").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("signal-types-gone");
  });

  it("resolves the five schema comments that point off the reference page", () => {
    // These read correctly on one page and broke the moment it became eight.
    // The fence keeps the reference's words; the note resolves them.
    expect(rpc).toContain("see Signal types below");
    expect(rpc).toContain('href="/console/entities.html#signal-types"');
    expect(rpc).toContain('href="/console/entities.html#memory-types"');
    expect(rpc).toContain('href="/console/entities.html#capability-bits"');
  });

  it("fails when a quoted reference to another page is left unresolved", () => {
    const res = withScript((t) => t.replace("  if (xref) parts.push(xref);", "  void xref;"));
    expect(res.landed, "the xref note was not disabled").toBe(true);
    expect(res.status).not.toBe(0);
    expect(res.output).toContain("unresolved");
  });

  it("no longer claims the network section converts blocks to time", () => {
    // The target page states the opposite, and that is a settled decision.
    expect(committed).not.toContain("the cadence in the network section converts it");
    expect(committed).toContain("It is not converted to wall-clock time anywhere on this console");
  });
});

describe("generate-console-html: the find surfaces carry their own identity", () => {
  it("ships no dead anchors and no inherited current-page marks", () => {
    for (const f of GENERATED_FILES) {
      const html = pages.get(f) as string;
      expect(html, `${f} still links to a landing-page fragment`).not.toContain('href="#connect"');
      expect(html, `${f} still links to a landing-page fragment`).not.toContain('href="#first-call"');
      expect(html, `${f} still marks a section as current`).not.toContain("aria-current");
    }
  });

  it("gives every constant row a fragment rather than only a page", () => {
    // Scoped to the index table. The shell's own page links (the header, the
    // canonical links) legitimately carry no fragment, and counting those would
    // measure the chrome rather than the index.
    const names = pages.get("console/names.html") as string;
    const table = names.slice(names.indexOf("<tbody>"), names.indexOf("</tbody>"));
    const rows = table.match(/href="[^"]*"/g) ?? [];
    const withFragment = rows.filter((h) => h.includes("#"));
    expect(rows.length).toBeGreaterThan(50);
    expect(withFragment.length, "an index row lands at the top of a page instead of at the name").toBe(rows.length);
  });

  it("harvests no constant out of a path or a filename", () => {
    const names = pages.get("console/names.html") as string;
    // RPC_REFERENCE occurs zero times in crates/ or sdk/. It was read out of
    // the path docs/RPC_REFERENCE.md in a provenance line.
    expect(names).not.toContain(">RPC_REFERENCE<");
  });

  it("names a destination that appears in the navigation", () => {
    // "get started" is a page label that appears in no nav on any page, so the
    // index pointed the reader somewhere they could not see a name for.
    expect(pages.get("console/names.html")).not.toContain("get started");
  });
});

describe("generate-console-html: the citation promise is measured", () => {
  it("renders as many citations as it claims", () => {
    const entities = pages.get("console/entities.html") as string;
    const claim = /([0-9]+) citations across ([0-9]+) declarations/.exec(entities);
    expect(claim, "the citation claim is not on the page").not.toBeNull();
    const recipes = entities.slice(entities.indexOf("a reputation oracle"), entities.indexOf("citations across"));
    const links = (recipes.match(/blob\/main/g) ?? []).length;
    expect(links, "the page claims more citations than it renders").toBe(Number((claim as RegExpExecArray)[1]));
  });

  it("no longer promises that every constant is cited without saying how many", () => {
    expect(committed).not.toContain("with every constant cited to its declaration");
  });
});

describe("generate-console-html: the chart caption states ties honestly", () => {
  it("names every type at the extremum rather than one of them", () => {
    const tx = pages.get("console/transactions.html") as string;
    const desc = /<desc id="fee-ladder-desc">([^<]*)</.exec(tx) as RegExpExecArray;
    expect(desc[1]).toContain("tie at");
    expect(desc[1]).toContain("50 times");
    // The defect was the definite article on a set of three.
    expect(desc[1]).not.toMatch(/The most expensive, \w+, costs 5,000/);
  });
});

describe("generate-console-html: the skip link moves focus", () => {
  it("gives every page a focusable main", () => {
    for (const [file, html] of pages) {
      expect(html, `${file} has no focusable <main>`).toContain('<main id="main" tabindex="-1"');
    }
  });
});
