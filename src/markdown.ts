type MarkdownNode = {
  type: string;
  value?: string;
  children?: MarkdownNode[];
};

const ambiguousStrong = /\*\*([^*\r\n]+?)([\p{P}\p{S}]+)\*\*(?=[\p{L}\p{N}])/gu;

export function remarkCjkStrongBoundaries() {
  return (tree: MarkdownNode) => rewriteChildren(tree);
}

function rewriteChildren(parent: MarkdownNode) {
  if (!parent.children) return;

  const children: MarkdownNode[] = [];
  for (const child of parent.children) {
    if (child.type === "text" && child.value) {
      children.push(...rewriteAmbiguousStrong(child.value));
    } else {
      rewriteChildren(child);
      children.push(child);
    }
  }
  parent.children = children;
}

function rewriteAmbiguousStrong(value: string): MarkdownNode[] {
  const nodes: MarkdownNode[] = [];
  let cursor = 0;

  for (const match of value.matchAll(ambiguousStrong)) {
    const index = match.index;
    const [source, content, punctuation] = match;
    if (index > cursor) nodes.push({ type: "text", value: value.slice(cursor, index) });
    nodes.push({
      type: "strong",
      children: [{ type: "text", value: content }],
    });
    nodes.push({ type: "text", value: punctuation });
    cursor = index + source.length;
  }

  if (cursor === 0) return [{ type: "text", value }];
  if (cursor < value.length) nodes.push({ type: "text", value: value.slice(cursor) });
  return nodes;
}
