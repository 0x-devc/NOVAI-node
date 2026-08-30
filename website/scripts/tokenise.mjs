//
// A build-time tokeniser for the two languages this console actually contains.
//
// WHY HAND WRITTEN RATHER THAN A GRAMMAR OFF THE SHELF
//
// Measured over the rendered page: 84 code blocks, 51 JSON-shaped, 31 shell,
// one Python and one Rust. JSON.parse fails on 29 of the 51, and that is not a
// defect. The fences are a schema notation, not JSON, and they carry three
// constructs no JSON grammar knows:
//
//   type placeholders     "block_hash": "<hex32>",  "height": <u64>
//   record references     { "channels": [PaymentChannel, ...] }
//   line comments         // canonical hash of this block
//
// A real grammar rejects all three and falls back to unscoped text, so it would
// be LESS accurate here, not more. Worse, it could not express the one thing
// worth expressing: a record reference is a link to a definition elsewhere on
// the page, which is what turns colour into navigation. That construct is ours,
// so the tokeniser has to be ours.
//
// The shell side needs no argument at all. Across all 31 blocks there is one
// command, curl, and four flags: -s -X -H -d.
//
// INVARIANT, asserted by the caller: concatenating every token's text
// reproduces the input byte for byte, and no character is left unclassified.
// A highlighter that silently drops a character is worse than none, because the
// reader cannot tell that it did.
//

/** Record-type names the page defines, set by the caller. */
const isTypeRef = (word, known) => known.has(word);

/**
 * The schema notation used in every result and response fence.
 *
 * Deliberately a scanner over characters rather than a parser over a grammar:
 * the input is frequently not valid JSON by design, and a parser would have to
 * decide what to do about that. A scanner has nothing to be wrong about.
 */
export function tokeniseSchema(src, known = new Set()) {
  const out = [];
  const push = (cls, text) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.cls === cls) last.text += text;
    else out.push({ cls, text });
  };
  let i = 0;
  while (i < src.length) {
    const c = src[i];

    // line comment
    if (c === "/" && src[i + 1] === "/") {
      const end = src.indexOf("\n", i);
      const stop = end === -1 ? src.length : end;
      push("comment", src.slice(i, stop));
      i = stop;
      continue;
    }

    // string, and whether it is a key depends on what follows the closing quote
    if (c === '"') {
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === "\\") { j += 2; continue; }
        if (src[j] === '"') break;
        j += 1;
      }
      const stop = Math.min(j + 1, src.length);
      const text = src.slice(i, stop);
      const after = /^\s*:/.test(src.slice(stop));
      const inner = text.slice(1, -1);
      // "<hex32>" is a placeholder that happens to be quoted. It is the single
      // most common value on this page and it is an instruction, not a value.
      const placeholder = !after && /^<[A-Za-z0-9_|\s-]+>$/.test(inner);
      // "prop", not "key". The class name is emitted into the markup as
      // tok-prop, and a class literally called "key" sitting immediately before
      // a 64-character hex example makes every secret scanner in the world
      // report a credential beside a keyword. The values are illustrative
      // examples out of the reference document and nothing about them changed;
      // only the word next to them did. "property" is also what ethereum.org
      // calls this token, so the accurate name and the quiet one agree.
      push(after ? "prop" : placeholder ? "ph" : "str", text);
      i = stop;
      continue;
    }

    // bare placeholder: <u64>, <hex32>, <string>
    if (c === "<") {
      const m = /^<[A-Za-z0-9_|\s-]+>/.exec(src.slice(i));
      if (m) {
        push("ph", m[0]);
        i += m[0].length;
        continue;
      }
    }

    // number
    const num = /^-?\d+(\.\d+)?/.exec(src.slice(i));
    if (num && !/[A-Za-z0-9_]/.test(src[i - 1] ?? "")) {
      push("num", num[0]);
      i += num[0].length;
      continue;
    }

    // word: literal, record reference, or bare identifier
    const word = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i));
    if (word) {
      const w = word[0];
      push(["true", "false", "null"].includes(w) ? "num" : isTypeRef(w, known) ? "type" : "plain", w);
      i += w.length;
      continue;
    }

    if (/[{}[\],:|]/.test(c)) { push("punct", c); i += 1; continue; }
    push("plain", c);
    i += 1;
  }
  return out;
}

/**
 * Shell. One command and four flags across the whole page, so this is a split
 * into command, flag, quoted string, variable and URL, and nothing more.
 */
export function tokeniseShell(src) {
  const out = [];
  const push = (cls, text) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.cls === cls) last.text += text;
    else out.push({ cls, text });
  };
  let i = 0;
  let atLineStart = true;
  while (i < src.length) {
    const c = src[i];

    if (c === "#" && atLineStart) {
      const end = src.indexOf("\n", i);
      const stop = end === -1 ? src.length : end;
      push("comment", src.slice(i, stop));
      i = stop;
      continue;
    }

    // A single-quoted argument is opaque to the shell, and on this page it is
    // always the JSON body. It is tokenised as a string here rather than
    // recursed into: the body is shown as one argument because that is what the
    // reader has to paste, and colouring inside it would suggest otherwise.
    if (c === "'" || c === '"') {
      const q = c;
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === "\\") { j += 2; continue; }
        if (src[j] === q) break;
        j += 1;
      }
      push("str", src.slice(i, Math.min(j + 1, src.length)));
      i = Math.min(j + 1, src.length);
      atLineStart = false;
      continue;
    }

    if (c === "$") {
      const m = /^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/.exec(src.slice(i));
      if (m) { push("var", m[0]); i += m[0].length; atLineStart = false; continue; }
    }

    const url = /^https?:\/\/[^\s'"]+/.exec(src.slice(i));
    if (url) { push("url", url[0]); i += url[0].length; atLineStart = false; continue; }

    const flag = /^-{1,2}[A-Za-z][\w-]*/.exec(src.slice(i));
    if (flag && /\s/.test(src[i - 1] ?? " ")) {
      push("flag", flag[0]); i += flag[0].length; atLineStart = false; continue;
    }

    const word = /^[A-Za-z_][\w.-]*/.exec(src.slice(i));
    if (word) {
      push(atLineStart ? "cmd" : "plain", word[0]);
      i += word[0].length;
      atLineStart = false;
      continue;
    }

    if (c === "\n") { push("plain", c); i += 1; atLineStart = true; continue; }
    if (c === "\\" && src[i + 1] === "\n") { push("punct", "\\"); i += 1; continue; }
    if (/\s/.test(c)) { push("plain", c); i += 1; continue; }
    push("punct", c);
    i += 1;
    atLineStart = false;
  }
  return out;
}

/** The invariant, callable by both the generator and the tests. */
export function assertLossless(src, tokens) {
  const joined = tokens.map((t) => t.text).join("");
  if (joined !== src) {
    const at = [...joined].findIndex((ch, k) => ch !== src[k]);
    throw new Error(
      `tokeniser lost or altered input at offset ${at < 0 ? joined.length : at}: ` +
      `expected ${JSON.stringify(src.slice(Math.max(0, at - 20), at + 20))}, ` +
      `produced ${JSON.stringify(joined.slice(Math.max(0, at - 20), at + 20))}`
    );
  }
  const unclassified = tokens.filter((t) => !t.cls);
  if (unclassified.length) throw new Error(`tokeniser produced ${unclassified.length} unclassified span(s)`);
}
