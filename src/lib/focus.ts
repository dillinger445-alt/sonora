/** Keyboard focus helpers. */

/**
 * Moves focus to the main region (keeping its scroll position). For when the
 * focused control is about to disappear, such as the remove button of a row
 * that was just removed, so focus does not fall back to the top of the page.
 */
export function focusMain(): void {
  document.getElementById('main')?.focus({ preventScroll: true });
}
