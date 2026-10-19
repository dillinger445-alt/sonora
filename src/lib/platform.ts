import type { Platform } from '../api/bridge.ts';

/** What the system's file manager calls showing a file. */
export function revealLabel(platform: Platform): string {
  if (platform === 'macos') return 'Reveal in Finder';
  if (platform === 'windows') return 'Show in File Explorer';
  return 'Show in folder';
}
