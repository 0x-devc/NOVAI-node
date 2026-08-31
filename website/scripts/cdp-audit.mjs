#!/usr/bin/env node
//
// Responsive and keyboard audit of the built console, driven over the Chrome
// DevTools Protocol.
//
// WHY THIS SCRIPT EXISTS AT ALL
//
// Every viewport, tab-order and focus figure this project has published was
// produced by a driver written inside a session and thrown away with it. The
// numbers were therefore unreproducible, on a page whose own report records that
// its first focus instrument gave two wrong readings in a row on one control.
// A measurement nobody can repeat is a claim. This is the instrument, committed.
//
// TWO INSTRUMENT ERRORS IT IS BUILT TO AVOID, BOTH PREVIOUSLY MADE HERE
//
//   1. Window sizing is not viewport sizing. Chrome on macOS clamps its window
//      to about 500px, so --window-size=360 reports overflow that is not real.
//      Only Emulation.setDeviceMetricsOverride sets a true viewport, and this
//      script reads window.innerWidth back on every run and fails if it differs
//      from what was asked for.
//
//   2. element.focus() does not reliably engage :focus-visible, which is what
//      the stylesheet keys its focus rings on. Measuring it that way reported 12
//      of 25 controls with no focus indicator when the real figure was zero.
//      This walks the tab order with real Input.dispatchKeyEvent Tab presses.
//
// It also settles before sampling. The copy button transitions opacity over
// 120ms, and a sample taken at 40ms reads 0.277, which is neither the start nor
// the end state and is not what any user sees.
//
// No dependencies: Node 22+ has a native WebSocket, and the pages are static, so
// they are served from a small http server here rather than through Vite. That
// keeps the audit to one process and one command.
//
// Usage:
//   npx vite build                       (dist/ must exist and be current)
//   node scripts/cdp-audit.mjs
//   node scripts/cdp-audit.mjs --dir dist --widths 360,768,1440,2560 --json
//
// Exit code is non-zero if any gating check fails.
//

import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { readFileSync, existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { join, resolve, extname, dirname } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = resolve(SCRIPT_DIR, "..");

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

// The three pages worth walking key by key. The tab order is generated from one
// shell, so walking all ten would re-measure the same navigation nine times;
// these three differ in what <main> contributes: copy buttons, a long method
// index, and a page with neither.
const KEYBOARD_PAGES = ["console.html", "console/rpc.html", "console/names.html"];

const CHROME_CANDIDATES = [
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
];

const MIME = new Map([
  [".html", "text/html; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".png", "image/png"],
  [".jpg", "image/jpeg"],
  [".webp", "image/webp"],
  [".woff2", "font/woff2"],
  [".ico", "image/x-icon"],
]);

function fail(msg) {
  console.error(`cdp-audit: FAIL: ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = { dir: "dist", widths: [360, 768, 1440, 2560], json: false, keepOpen: false };
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === "--dir") args.dir = argv[++i];
    else if (argv[i] === "--widths") args.widths = argv[++i].split(",").map((w) => Number(w.trim()));
    else if (argv[i] === "--json") args.json = true;
    else fail(`unknown argument: ${argv[i]}`);
  }
  if (args.widths.some((w) => !Number.isFinite(w) || w <= 0)) fail("--widths must be positive numbers");
  return args;
}

// ---------------------------------------------------------------- static host

function serve(root) {
  return new Promise((ready) => {
    const server = createServer((req, res) => {
      const url = decodeURIComponent((req.url ?? "/").split("?")[0]);
      let rel = url === "/" ? "/index.html" : url;
      const path = join(root, rel);
      // Containment: a served path must stay inside the root even if the request
      // walks up. Without this the audit host would read the whole disk.
      if (!resolve(path).startsWith(resolve(root))) {
        res.writeHead(403).end("forbidden");
        return;
      }
      if (!existsSync(path) || !statSync(path).isFile()) {
        res.writeHead(404).end("not found");
        return;
      }
      res.writeHead(200, { "content-type": MIME.get(extname(path)) ?? "application/octet-stream" });
      res.end(readFileSync(path));
    });
    server.listen(0, "127.0.0.1", () => ready({ server, port: server.address().port }));
  });
}

// ------------------------------------------------------------------- cdp glue

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForJson(url, attempts = 100) {
  for (let i = 0; i < attempts; i++) {
    try {
      const res = await fetch(url);
      if (res.ok) return await res.json();
    } catch {
      /* not up yet */
    }
    await sleep(100);
  }
  fail(`the browser never answered ${url}; it may have failed to start`);
}

/** A flattened CDP session: one socket, session-scoped messages, awaited by id. */
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.listeners = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id !== undefined && this.pending.has(msg.id)) {
        const { ok, no } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) no(new Error(`${msg.error.message} (${JSON.stringify(msg.error.data ?? null)})`));
        else ok(msg.result);
        return;
      }
      const subs = this.listeners.get(msg.method);
      if (subs) for (const fn of [...subs]) fn(msg.params);
    });
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    this.ws.send(JSON.stringify(payload));
    return new Promise((ok, no) => {
      this.pending.set(id, { ok, no });
      setTimeout(() => {
        if (this.pending.delete(id)) no(new Error(`${method} timed out after 30s`));
      }, 30_000);
    });
  }

  once(method) {
    return new Promise((ok) => {
      const fn = (params) => {
        this.off(method, fn);
        ok(params);
      };
      this.on(method, fn);
    });
  }

  on(method, fn) {
    if (!this.listeners.has(method)) this.listeners.set(method, new Set());
    this.listeners.get(method).add(fn);
  }

  off(method, fn) {
    this.listeners.get(method)?.delete(fn);
  }
}

async function launch() {
  const bin = CHROME_CANDIDATES.find((p) => existsSync(p));
  if (!bin) fail(`no Chrome-family browser found. Looked in:\n  ${CHROME_CANDIDATES.join("\n  ")}`);
  const profile = mkdtempSync(join(tmpdir(), "cdp-audit-"));
  const proc = spawn(
    bin,
    [
      "--headless=new",
      "--remote-debugging-port=0",
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-extensions",
      "--disable-gpu",
      "--hide-scrollbars",
      "about:blank",
    ],
    { stdio: ["ignore", "pipe", "pipe"] }
  );

  // The port is written to DevToolsActivePort in the profile once the browser is
  // listening. Reading it is what makes --remote-debugging-port=0 usable and
  // keeps parallel runs from colliding on a fixed port.
  let port = null;
  const portFile = join(profile, "DevToolsActivePort");
  for (let i = 0; i < 100 && port === null; i++) {
    if (existsSync(portFile)) {
      const first = readFileSync(portFile, "utf8").split("\n")[0].trim();
      if (first) port = Number(first);
    }
    if (port === null) await sleep(100);
  }
  if (!port) {
    proc.kill("SIGKILL");
    fail("the browser did not report a debugging port");
  }

  const version = await waitForJson(`http://127.0.0.1:${port}/json/version`);
  const ws = new WebSocket(version.webSocketDebuggerUrl);
  await new Promise((ok, no) => {
    ws.addEventListener("open", ok, { once: true });
    ws.addEventListener("error", () => no(new Error("could not open the CDP socket")), { once: true });
  });

  const cdp = new Cdp(ws);
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  await cdp.send("Page.enable", {}, sessionId);
  await cdp.send("Runtime.enable", {}, sessionId);

  const close = () => {
    try {
      ws.close();
    } catch {
      /* already gone */
    }
    proc.kill("SIGKILL");
    rmSync(profile, { recursive: true, force: true });
  };

  return { cdp, sessionId, close, browser: version.Browser };
}

async function evaluate(cdp, sessionId, expression) {
  const res = await cdp.send(
    "Runtime.evaluate",
    { expression, returnByValue: true, awaitPromise: true },
    sessionId
  );
  if (res.exceptionDetails) {
    throw new Error(res.exceptionDetails.exception?.description ?? res.exceptionDetails.text);
  }
  return res.result.value;
}

async function goto(cdp, sessionId, url) {
  const loaded = cdp.once("Page.loadEventFired");
  await cdp.send("Page.navigate", { url }, sessionId);
  await loaded;
  // Fonts and the stylesheet settle after load; layout read before that reports
  // widths that no reader ever sees.
  await evaluate(cdp, sessionId, "document.fonts ? document.fonts.ready.then(() => true) : true");
  await sleep(120);
}

async function pressTab(cdp, sessionId) {
  for (const type of ["rawKeyDown", "keyUp"]) {
    await cdp.send(
      "Input.dispatchKeyEvent",
      { type, key: "Tab", code: "Tab", windowsVirtualKeyCode: 9, nativeVirtualKeyCode: 9 },
      sessionId
    );
  }
  // The focus ring and the copy button both transition over 120ms. Sampling
  // before that settles reads an intermediate opacity that is not a real state.
  await sleep(200);
}

// ------------------------------------------------------------------ measuring

const LAYOUT_PROBE = `(() => {
  const main = document.querySelector('main#main');
  if (!main) return { error: 'no main#main' };
  const cs = getComputedStyle(main.parentElement);
  const rail = document.querySelector('.console-rail');
  const chip = document.querySelector('.console-chip');
  const vis = (el) => {
    if (!el) return 'absent';
    const s = getComputedStyle(el.closest('nav') ?? el);
    return s.display === 'none' ? 'hidden' : 'shown';
  };
  return {
    innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    bodyScrollWidth: document.body.scrollWidth,
    mainWidth: Math.round(main.getBoundingClientRect().width),
    layoutDisplay: cs.display,
    rail: vis(rail),
    chips: vis(chip),
  };
})()`;

const SEMANTICS_PROBE = `(() => {
  const heads = [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].map((h) => Number(h.tagName[1]));
  let skips = [];
  for (let i = 1; i < heads.length; i++) if (heads[i] - heads[i - 1] > 1) skips.push(heads[i - 1] + '->' + heads[i]);
  const navs = [...document.querySelectorAll('nav')];
  const visible = navs.filter((n) => getComputedStyle(n).display !== 'none');
  const labels = visible.map((n) => n.getAttribute('aria-label') ?? '');
  return {
    h1: document.querySelectorAll('h1').length,
    headings: heads.length,
    skips,
    mains: document.querySelectorAll('main').length,
    navs: navs.length,
    unlabelledNavs: navs.filter((n) => !n.getAttribute('aria-label') && !n.getAttribute('aria-labelledby')).length,
    visibleNavs: visible.length,
    visibleNavLabels: labels,
    duplicateNavLabels: labels.filter((l, i) => labels.indexOf(l) !== i),
    ariaCurrent: [...document.querySelectorAll('[aria-current]')].map((el) => el.getAttribute('href')),
    danglingFragments: [...document.querySelectorAll('a[href^="#"]')]
      .map((a) => a.getAttribute('href').slice(1))
      .filter((id) => id && !document.getElementById(id)),
    mainTabindex: document.querySelector('main#main')?.getAttribute('tabindex') ?? null,
  };
})()`;

const ACTIVE_PROBE = `(() => {
  const el = document.activeElement;
  if (!el || el === document.body) return null;
  const cs = getComputedStyle(el);
  let focusVisible = false;
  try { focusVisible = el.matches(':focus-visible'); } catch { focusVisible = false; }
  const r = el.getBoundingClientRect();
  return {
    tag: el.tagName.toLowerCase(),
    cls: (el.getAttribute('class') ?? '').split(/\\s+/).filter(Boolean).slice(0, 3).join(' '),
    text: (el.textContent ?? '').trim().slice(0, 40),
    href: el.getAttribute('href'),
    focusVisible,
    opacity: Number(cs.opacity),
    outlineWidth: cs.outlineWidth,
    boxShadow: cs.boxShadow === 'none' ? '' : 'set',
    width: Math.round(r.width),
    height: Math.round(r.height),
  };
})()`;

/** A focused control a keyboard user can reach must be perceivable when focused. */
const hasIndicator = (a) =>
  a.opacity > 0.99 && (a.outlineWidth !== "0px" || a.boxShadow === "set" || a.tag === "a" || a.tag === "button");

async function main() {
  const args = parseArgs(process.argv);
  const root = resolve(WEB_ROOT, args.dir);
  if (!existsSync(root)) {
    fail(`${args.dir} does not exist. Build first: npx vite build`);
  }
  for (const p of PAGES) {
    if (!existsSync(join(root, p))) fail(`${args.dir}/${p} is missing, so the audit would silently skip a page`);
  }

  const { server, port } = await serve(root);
  const { cdp, sessionId, close, browser } = await launch();
  const problems = [];
  const viewportRows = [];
  const semanticRows = [];
  const keyboardRows = [];

  try {
    for (const width of args.widths) {
      for (const page of PAGES) {
        await cdp.send(
          "Emulation.setDeviceMetricsOverride",
          { width, height: 900, deviceScaleFactor: 1, mobile: width < 768 },
          sessionId
        );
        await goto(cdp, sessionId, `http://127.0.0.1:${port}/${page}`);
        const m = await evaluate(cdp, sessionId, LAYOUT_PROBE);
        if (m.error) {
          problems.push(`${page} @ ${width}: ${m.error}`);
          continue;
        }
        // The instrument checks itself before its reading is used. If the
        // emulated viewport is not the width that was asked for, every overflow
        // number below it is meaningless.
        if (m.innerWidth !== width) {
          problems.push(`${page} @ ${width}: innerWidth read back as ${m.innerWidth}, so the viewport override did not take`);
        }
        if (m.scrollWidth > m.innerWidth) {
          problems.push(`${page} @ ${width}: document overflows, scrollWidth ${m.scrollWidth} against innerWidth ${m.innerWidth}`);
        }
        viewportRows.push({ page, width, ...m });

        if (width === args.widths[args.widths.length - 1]) {
          const s = await evaluate(cdp, sessionId, SEMANTICS_PROBE);
          if (s.h1 !== 1) problems.push(`${page}: ${s.h1} h1 elements, expected exactly 1`);
          if (s.skips.length) problems.push(`${page}: heading level skipped (${s.skips.join(", ")})`);
          if (s.mains !== 1) problems.push(`${page}: ${s.mains} main elements, expected exactly 1`);
          if (s.unlabelledNavs) problems.push(`${page}: ${s.unlabelledNavs} unlabelled nav landmark(s)`);
          // Several visible navs are fine and expected; two carrying the SAME
          // label are not, because a screen reader listing landmarks then offers
          // two indistinguishable choices. The rail and the chips both label
          // themselves "Sections" and are mutually exclusive by media query, so
          // this must compare labels among the visible ones rather than count
          // them. Counting was the first version of this check and it fired on
          // all ten pages against a page that is correct.
          if (s.duplicateNavLabels.length) {
            problems.push(`${page}: ${s.duplicateNavLabels.length} visible nav(s) share a label: ${[...new Set(s.duplicateNavLabels)].join(", ")}`);
          }
          if (s.danglingFragments.length) {
            problems.push(`${page}: same-page link(s) to no such id: ${[...new Set(s.danglingFragments)].join(", ")}`);
          }
          // A skip link that moves the scroll but not the focus leaves a screen
          // reader user reading from where they already were, which is the whole
          // thing the link exists to prevent. <main> needs tabindex="-1" to be a
          // focus target at all.
          if (s.mainTabindex !== "-1") {
            problems.push(`${page}: <main id="main"> has tabindex ${s.mainTabindex ?? "unset"}, so the skip link moves the scroll but not the focus`);
          }
          // The two generated find surfaces own no section, so nothing on them
          // is the current page. They inherit their shell from the landing page,
          // which is how they came to claim otherwise.
          if ((page === "console/all.html" || page === "console/names.html") && s.ariaCurrent.length) {
            problems.push(`${page}: ${s.ariaCurrent.length} aria-current mark(s) on a page that owns no section`);
          }
          semanticRows.push({ page, ...s });
        }
      }
    }

    // Keyboard, at the widest viewport, where both nav forms and every control
    // are laid out normally.
    await cdp.send(
      "Emulation.setDeviceMetricsOverride",
      { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false },
      sessionId
    );
    for (const page of KEYBOARD_PAGES) {
      await goto(cdp, sessionId, `http://127.0.0.1:${port}/${page}`);
      await evaluate(cdp, sessionId, "document.body.focus(); document.activeElement === document.body");
      const stops = [];
      const seen = new Set();
      for (let i = 0; i < 220; i++) {
        await pressTab(cdp, sessionId);
        const a = await evaluate(cdp, sessionId, ACTIVE_PROBE);
        if (!a) break;
        const key = `${a.tag}|${a.cls}|${a.href}|${a.text}`;
        if (seen.has(key) && stops.length > 3) break; // wrapped round
        seen.add(key);
        stops.push(a);
      }
      const noIndicator = stops.filter((a) => !a.focusVisible || !hasIndicator(a));
      if (stops.length === 0) problems.push(`${page}: no focusable stops found, so the keyboard walk measured nothing`);
      if (noIndicator.length) {
        for (const a of noIndicator.slice(0, 6)) {
          problems.push(`${page}: focus stop <${a.tag} class="${a.cls}"> has no visible indicator (focusVisible=${a.focusVisible}, opacity=${a.opacity}, outline=${a.outlineWidth})`);
        }
      }
      const first = stops[0];
      if (!first || first.href !== "#main") {
        problems.push(`${page}: the first tab stop is not the skip link (got ${first ? `${first.tag} ${first.href ?? ""}` : "nothing"})`);
      }
      keyboardRows.push({ page, stops: stops.length, withoutIndicator: noIndicator.length, firstStop: first?.href ?? null });
    }
  } finally {
    close();
    server.close();
  }

  const report = { browser, widths: args.widths, viewportRows, semanticRows, keyboardRows, problems };
  if (args.json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log(`cdp-audit: ${browser}, serving ${args.dir}\n`);
    console.log("viewports (document overflow is the gating check)");
    for (const w of args.widths) {
      const rows = viewportRows.filter((r) => r.width === w);
      const over = rows.filter((r) => r.scrollWidth > r.innerWidth).length;
      const mains = [...new Set(rows.map((r) => r.mainWidth))].sort((a, b) => a - b);
      const disp = [...new Set(rows.map((r) => r.layoutDisplay))].join("/");
      const rail = [...new Set(rows.map((r) => r.rail))].join("/");
      const chips = [...new Set(rows.map((r) => r.chips))].join("/");
      console.log(
        `  ${String(w).padStart(4)}  overflow ${over}/${rows.length}  main ${mains.join(",")}  layout ${disp}  rail ${rail}  chips ${chips}`
      );
    }
    console.log("\nsemantics (at the widest viewport)");
    for (const s of semanticRows) {
      console.log(
        `  ${s.page.padEnd(26)} h1 ${s.h1}  headings ${String(s.headings).padStart(3)}  skips ${s.skips.length}  navs ${s.navs} (${s.visibleNavs} visible)  aria-current ${s.ariaCurrent.length}  dangling ${s.danglingFragments.length}  main tabindex ${s.mainTabindex ?? "none"}`
      );
    }
    console.log("\nkeyboard (real Tab keys, sampled after transitions settle)");
    for (const k of keyboardRows) {
      console.log(`  ${k.page.padEnd(26)} ${String(k.stops).padStart(3)} stops, ${k.withoutIndicator} without an indicator, first stop ${k.firstStop ?? "none"}`);
    }
  }

  if (problems.length) {
    console.error(`\ncdp-audit: ${problems.length} problem(s):`);
    for (const p of problems) console.error(`  ${p}`);
    fail(`${problems.length} accessibility or layout problem(s)`);
  }
  console.log(`\ncdp-audit: clean (${viewportRows.length} page-width measurements, ${keyboardRows.reduce((n, k) => n + k.stops, 0)} focus stops walked)`);
}

main().catch((e) => fail(e.stack ?? String(e)));
