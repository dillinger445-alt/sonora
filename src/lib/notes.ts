/**
 * Release notes (GitHub Markdown) split into the few block kinds SONORA
 * shows: headings, paragraphs and lists. Inline marks are left in the text.
 */

export type NoteBlock = { kind: 'h'; text: string } | { kind: 'p'; text: string } | { kind: 'ul' | 'ol'; items: string[] };

export function parseNotes(md: string): NoteBlock[] {
  const blocks: NoteBlock[] = [];
  let para: string[] = [];
  // A line right after a list item (no blank line between) continues it.
  let inItem = false;
  const flush = () => {
    if (para.length) blocks.push({ kind: 'p', text: para.join(' ') });
    para = [];
  };
  for (const raw of md.replace(/\r\n?/g, '\n').split('\n')) {
    const line = raw.trim();
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    const bullet = /^[-*+]\s+(.*)$/.exec(line);
    const numbered = /^\d+[.)]\s+(.*)$/.exec(line);
    if (!line) {
      flush();
      inItem = false;
    } else if (heading) {
      flush();
      inItem = false;
      blocks.push({ kind: 'h', text: heading[1] ?? '' });
    } else if (bullet || numbered) {
      flush();
      const kind = bullet ? 'ul' : 'ol';
      const text = (bullet ?? numbered)?.[1] ?? '';
      const prev = blocks[blocks.length - 1];
      if (prev && prev.kind === kind) prev.items.push(text);
      else blocks.push({ kind, items: [text] });
      inItem = true;
    } else if (/^(-{3,}|\*{3,}|_{3,})$/.test(line)) {
      flush();
      inItem = false;
    } else if (inItem) {
      const prev = blocks[blocks.length - 1];
      if (prev && (prev.kind === 'ul' || prev.kind === 'ol')) prev.items[prev.items.length - 1] += ` ${line}`;
    } else {
      para.push(line);
    }
  }
  flush();
  return blocks;
}
