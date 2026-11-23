// Bun currently provides decorator context.metadata but not the Symbol.metadata key.
(Symbol as unknown as { metadata: symbol }).metadata ??= Symbol.for('Symbol.metadata');

/** The `Symbol.metadata` key, polyfilled above for runtimes that lack it. */
export const metadataKey: symbol = (Symbol as unknown as { metadata: symbol }).metadata;
