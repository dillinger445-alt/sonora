/** Keyboard focus helpers. */

/**
 * Moves focus to the main region (keeping its scroll position). For when the
 * focused control is about to disappear, such as the remove button of a row
 * that was just removed, so focus does not fall back to the top of the page.
 */
export function focusMain(): void {
  document.getElementById('main')?.focus({ preventScroll: true });
}

/** Input types that take no typing: Backspace there is not an edit. */
const NOT_TEXT = new Set(['checkbox', 'radio', 'button', 'submit', 'reset', 'range', 'color', 'file', 'image', 'hidden']);

/** Whether keys pressed at `target` edit text there (a text field, a text area, editable content). */
export function isTextEntry(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target.tagName === 'TEXTAREA') return true;
  return target instanceof HTMLInputElement && !NOT_TEXT.has(target.type);
}

/**
 * Whether keyboard focus has nowhere useful to be: on the page itself, on a
 * control that is gone, or on the main region. WebKit (the macOS app) does
 * not focus a button when it is clicked but the nearest focusable element
 * around it, which is the main region.
 */
export function focusIsLost(): boolean {
  const now = document.activeElement;
  return now == null || now === document.body || !now.isConnected || now === document.getElementById('main');
}
