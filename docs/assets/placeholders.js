// Marks oboro placeholders inside rendered code blocks.
//
// A redaction is what the tool produces, so the examples on this site draw it
// as a deliberate object rather than leaving it as incidental punctuation.
//
// Presentation only. If this never runs, every page still reads correctly,
// which is why it walks text nodes and changes nothing else.
(() => {
  const PLACEHOLDER = /\[\[[A-Z][A-Z0-9_]*_\d+\]\]/g;

  const mark = (textNode) => {
    const text = textNode.nodeValue;
    if (!text.includes("[[")) return;

    PLACEHOLDER.lastIndex = 0;
    if (!PLACEHOLDER.test(text)) return;

    PLACEHOLDER.lastIndex = 0;
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let match;

    while ((match = PLACEHOLDER.exec(text)) !== null) {
      if (match.index > cursor) {
        fragment.append(text.slice(cursor, match.index));
      }
      const span = document.createElement("span");
      span.className = "oboro-placeholder";
      span.textContent = match[0];
      fragment.append(span);
      cursor = match.index + match[0].length;
    }

    if (cursor < text.length) {
      fragment.append(text.slice(cursor));
    }
    textNode.replaceWith(fragment);
  };

  const run = () => {
    for (const block of document.querySelectorAll("pre code, code")) {
      // Collected first: replacing a node while walking would invalidate
      // the iterator part way through.
      const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT);
      const nodes = [];
      while (walker.nextNode()) nodes.push(walker.currentNode);
      nodes.forEach(mark);
    }
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", run);
  } else {
    run();
  }
})();
