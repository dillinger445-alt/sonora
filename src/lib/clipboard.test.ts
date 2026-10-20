import assert from 'node:assert/strict';
import { afterEach, test } from 'node:test';
import { copyText } from './clipboard.ts';

/** A stand-in clipboard that notes what it is asked, in order. */
function fakeClipboard(withItems: boolean) {
  const calls: { how: 'writeText' | 'write'; value: unknown }[] = [];
  const clipboard = {
    writeText: async (t: string) => {
      calls.push({ how: 'writeText', value: t });
    },
    ...(withItems
      ? {
          write: async (items: { types: Record<string, Promise<Blob>> }[]) => {
            calls.push({ how: 'write', value: items });
          },
        }
      : {}),
  };
  Object.defineProperty(globalThis, 'navigator', { value: { clipboard }, configurable: true });
  if (withItems) {
    Object.defineProperty(globalThis, 'ClipboardItem', {
      value: class {
        types: Record<string, Promise<Blob>>;
        constructor(types: Record<string, Promise<Blob>>) {
          this.types = types;
        }
      },
      configurable: true,
    });
  } else {
    Reflect.deleteProperty(globalThis, 'ClipboardItem');
  }
  return calls;
}

const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
afterEach(() => {
  if (original) Object.defineProperty(globalThis, 'navigator', original);
  Reflect.deleteProperty(globalThis, 'ClipboardItem');
});

test('text still on its way is handed to the clipboard at once, as a promise', async () => {
  const calls = fakeClipboard(true);
  let arrive: (t: string) => void = () => undefined;
  const text = new Promise<string>((resolve) => (arrive = resolve));
  const copied = copyText(text);
  // Asked while the click is being handled, before the text is there.
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.how, 'write');
  arrive('s3cret-web-password');
  await copied;
  const [item] = calls[0]?.value as { types: Record<string, Promise<Blob>> }[];
  const blob = await item?.types['text/plain'];
  assert.equal(blob?.type, 'text/plain');
  assert.equal(await blob?.text(), 's3cret-web-password');
});

test('text that is there already is written as it is', async () => {
  const calls = fakeClipboard(true);
  await copyText('ready');
  assert.deepEqual(calls, [{ how: 'writeText', value: 'ready' }]);
});

test('without clipboard items, the text is written once it arrives', async () => {
  const calls = fakeClipboard(false);
  const copied = copyText(Promise.resolve('later'));
  assert.equal(calls.length, 0);
  await copied;
  assert.deepEqual(calls, [{ how: 'writeText', value: 'later' }]);
});

test('text that never arrives is not copied, and says why', async () => {
  const calls = fakeClipboard(false);
  await assert.rejects(copyText(Promise.reject(new Error('slskd’s settings have no web password.'))), /no web password/);
  assert.equal(calls.length, 0);
});
