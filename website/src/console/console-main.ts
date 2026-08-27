// Console entry.
//
// The console's reference sections are static HTML in console.html and are
// never React: one rendering path per section, so there is no hydration
// boundary that could mismatch and no second copy that could drift.
//
// This entry therefore does almost nothing. It exists to pull in the
// stylesheets, which Vite extracts into a real <link> in the built HTML, so
// the page is fully styled with JavaScript disabled.
//
// From gate C4 it also mounts the two React islands, the live network panel
// and the verify panel, into their containers. Those mount rather than
// hydrate: the static markup is replaced by an identical first React paint,
// which removes hydration mismatch as a failure mode entirely.

import "../index.css";
import "./console.css";
