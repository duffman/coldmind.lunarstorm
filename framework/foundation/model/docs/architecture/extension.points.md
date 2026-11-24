# Extension points

What to touch for each kind of change. Every item ends with the same two steps: add the export
to `index.ts` (or `internal.ts`), and update `API_SPEC.md` and `__test__/model.test.ts`.

## A new field kind

1. `fields/<kind>.field.ts`: a class extending `Field<T, O>`, or `ConstrainedField<T, O>` when the
   kind has declarative constraints. Implement:
   - `clone()`: construct the same subclass, call `_copyBase(c)`, for a `ConstrainedField` also
     `_copyConstraints(c)`, and copy any state the class adds (`ListField` copies `_minItems` /
     `_maxItems`, `ObjField` copies `_strict`).
   - `_checkType(v)`: the base type test. `false` produces `TYPE`.
   - `_parseSync(path, v)`: structural checks; return `{ value, errors, warnings }`.
   - `_parseAsync` and `hasAsync()` when the field contains other fields.
   - `optional()`, `nullable()`, `required()`, `default()` overrides that cast `super.…()` to the
     subclass, so the fluent chain keeps the subclass's methods.
2. A factory function in the same file.
3. `fields/index.ts`: export the class and the factory.
4. `src/schema/model.schema.introspection.ts`: add the kind to `FieldKind` and to `kindOf`. `kindOf` is an
   `instanceof` chain that ends in `'any'`, so a kind left out is reported as `'any'`. Add
   kind-specific data in `introspectField` if the kind has any.
5. `src/schema/model.schema.json.ts`: add a `case` to the `switch` in `fieldToSchema`. The default branch
   emits `additionalProperties: true` with no `type`.
6. Check every consumer that switches on `FieldKind`.

## A constraint on `str` or `num`

Add a method that returns `this._addC(check, msg, code, level?, params?)`.

`params` is what `introspect` reports and what `toJsonSchema` reads. `applyParams` in
`src/schema/model.schema.json.ts` understands these keys and ignores the rest:

| Key | Effect |
|---|---|
| `type: 'integer'` | sets `type` |
| `format` | sets `format` |
| `enum` | intersects with an earlier `enum` |
| `pattern` (+ `flags`) | first goes to `pattern`, later ones to `allOf`; skipped when `flags` is non-empty |
| `minLength`, `minimum`, `exclusiveMinimum`, `minItems` | keeps the larger |
| `maxLength`, `maximum`, `exclusiveMaximum`, `maxItems` | keeps the smaller |

Add the code to the error-code table in `API_SPEC.md`.

## A check that is not declarative

Use `field.use(fn)` or `field.useAsync(fn)`. These run only when the structural parse produced
no errors, and they are not visible to `introspect` or JSON Schema.

## A data type

One file, one `defineDataType` call. See [../fields/data.types.md](../fields/data.types.md).

## A projection

In `src/models/model.projections.ts`:

```ts
export function mine<X extends ShapeInput>(shape: X): Model<NameOf<X>, …> {
  const { name, fields, strict } = metaOf(shape)
  const out: Shape = {}
  // build `out` from `fields`
  return buildModel(name, out, strict) as Model<NameOf<X>, …>
}
```

Pass `name` and `strict` through; every existing projection does. For handle parameters use
`RefTuple` for the type and `refKeys(fn, name, fields, refs)` for the runtime key set.

## A field annotation from another package

`FieldOpts<T>` is a named interface so another package can declaration-merge its own keys into
it. `_copyBase` spreads `_opts`, so a merged key survives every modifier and clone. `DataTypeSpec`
is open in the same way.

## Public or internal

`index.ts` is the public surface. `internal.ts` is for sibling `@coldmind/*` packages and tests:
put a name there when another package needs it but an application should not.
