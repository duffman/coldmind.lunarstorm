// src/core/cminject.lazy.ts
var deferredKind = Symbol("inject.deferred");
var declarations = new WeakSet;
function lazy(token) {
  const marker = Object.freeze({ token, [deferredKind]: "lazy" });
  declarations.add(marker);
  return marker;
}
function provider(token) {
  const marker = Object.freeze({ token, [deferredKind]: "provider" });
  declarations.add(marker);
  return marker;
}
function isDeferredToken(value) {
  return typeof value === "object" && value !== null && declarations.has(value);
}
function isLazyToken(value) {
  return isDeferredToken(value) && value[deferredKind] === "lazy";
}
function isProviderToken(value) {
  return isDeferredToken(value) && value[deferredKind] === "provider";
}
function unwrapDependencySlot(slot) {
  return isDeferredToken(slot) ? slot.token : slot;
}
function describeDependencySlot(slot) {
  return { token: unwrapDependencySlot(slot), kind: isDeferredToken(slot) ? slot[deferredKind] : "eager" };
}
function bindDeferredSlot(scope, slot) {
  if (!isDeferredToken(slot))
    throw new TypeError("Expected a lazy or provider dependency declaration");
  const get = async () => scope.resolve(slot.token);
  return isProviderToken(slot) ? get : Object.freeze({ get, getSync: () => scope.resolveSync(slot.token), peek: () => scope.tryGet(slot.token) });
}
export {
  unwrapDependencySlot,
  provider,
  lazy,
  isProviderToken,
  isLazyToken,
  isDeferredToken,
  describeDependencySlot,
  bindDeferredSlot
};
