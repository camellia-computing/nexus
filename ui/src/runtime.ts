const nativeHost = (() => {
  const internals = Reflect.get(window, '__TAURI_INTERNALS__');
  return !!internals
    && typeof internals === 'object'
    && Reflect.has(internals, 'metadata');
})();

export function isNativeHost() {
  return nativeHost;
}
