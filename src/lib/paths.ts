/** Path helpers for the typed-path import dialog (browser development). */

/** Accepts absolute POSIX paths, Windows drive paths and UNC paths. */
export function isAbsolutePath(p: string): boolean {
  return p.startsWith('/') || /^[A-Za-z]:[\\/]/.test(p) || p.startsWith('\\\\');
}

/** One path per line; surrounding quotes are dropped, duplicates removed. */
export function parsePathList(text: string): { paths: string[]; invalid: string[] } {
  const lines = text
    .split(/\r?\n/)
    .map((l) => l.trim().replace(/^["']|["']$/g, ''))
    .filter(Boolean);
  const paths: string[] = [];
  const invalid: string[] = [];
  for (const l of lines) {
    if (isAbsolutePath(l)) {
      if (!paths.includes(l)) paths.push(l);
    } else invalid.push(l);
  }
  return { paths, invalid };
}
