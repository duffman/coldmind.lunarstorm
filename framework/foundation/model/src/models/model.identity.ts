/**
 * @module model.identity
 * The MODEL symbol. Separated to break the cycle: model.define imports it,
 * and internal re-exports from model.define.
 */

/** The key a model's name and shape hang off; no field name can collide with it. */
export const MODEL: unique symbol = Symbol('modellera.model')

/** @internal Runtime owner token carried by field handles. */
export const MODEL_OWNER: unique symbol = Symbol('modellera.model.owner')
