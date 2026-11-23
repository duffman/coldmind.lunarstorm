/**
 * @module @coldmind/http-routes — HTTP route contracts in the shape of rpc contracts. `httpContract` declares a
 * group of routes (method, path with `:param` and `*rest`, schemas for params/query/headers/body/result,
 * access, docs); `RouteTable` matches requests with a fixed precedence; `procContractOf` converts a group
 * to the rpc contract a server registers; decorators declare a group as a class; `describeRoutes` returns
 * the group as JSON. No I/O.
 */
export * from "@coldmind/http-routes/src/routes.path.ts";
export * from "@coldmind/http-routes/src/routes.contract.ts";
export * from "@coldmind/http-routes/src/routes.table.ts";
export * from "@coldmind/http-routes/src/routes.coerce.ts";
export * from "@coldmind/http-routes/src/routes.decorators.ts";
export * from "@coldmind/http-routes/src/routes.describe.ts";
