/**
 * Release notes as written on GitHub, shown as text: headings, lists,
 * paragraphs, **bold** and `code`. Links show their text only, and nothing
 * in the notes is ever treated as HTML.
 */

import type { ReactNode } from 'react';
import { parseNotes } from '../lib/notes.ts';

function inline(text: string): ReactNode[] {
  // Links → their text; bare URLs stay as written.
  const plain = text.replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1');
  const out: ReactNode[] = [];
  const re = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  let last = 0;
  for (const m of plain.matchAll(re)) {
    const at = m.index ?? 0;
    if (at > last) out.push(plain.slice(last, at));
    const tok = m[0];
    out.push(tok.startsWith('**') ? <strong key={at}>{tok.slice(2, -2)}</strong> : <code key={at}>{tok.slice(1, -1)}</code>);
    last = at + tok.length;
  }
  if (last < plain.length) out.push(plain.slice(last));
  return out;
}

export function ReleaseNotes({ text }: { text: string }) {
  const blocks = parseNotes(text);
  if (!blocks.length) return null;
  return (
    <div className="notes selectable">
      {blocks.map((b, i) =>
        b.kind === 'h' ? (
          <h3 key={i} className="notes-h">
            {inline(b.text)}
          </h3>
        ) : b.kind === 'p' ? (
          <p key={i}>{inline(b.text)}</p>
        ) : b.kind === 'ul' ? (
          <ul key={i}>
            {b.items.map((t, j) => (
              <li key={j}>{inline(t)}</li>
            ))}
          </ul>
        ) : (
          <ol key={i}>
            {b.items.map((t, j) => (
              <li key={j}>{inline(t)}</li>
            ))}
          </ol>
        ),
      )}
    </div>
  );
}
