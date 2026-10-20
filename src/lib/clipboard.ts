/**
 * Copying text to the clipboard.
 *
 * WebKit, which shows the macOS app, lets a page write to the clipboard
 * only while it handles a click or a key press, and an answer from SONORA's
 * core can arrive after that. Text that is still on its way therefore goes
 * to the clipboard as a promise, handed over at once: call `copyText`
 * before the first `await` in the handler.
 */
export function copyText(text: string | Promise<string>): Promise<void> {
  const clipboard = globalThis.navigator?.clipboard;
  if (!clipboard) return Promise.reject(new Error('This window cannot use the clipboard.'));
  if (typeof text === 'string') return clipboard.writeText(text);
  if (typeof ClipboardItem === 'function' && typeof clipboard.write === 'function') {
    const blob = text.then((t) => new Blob([t], { type: 'text/plain' }));
    return clipboard.write([new ClipboardItem({ 'text/plain': blob })]);
  }
  return text.then((t) => clipboard.writeText(t));
}
