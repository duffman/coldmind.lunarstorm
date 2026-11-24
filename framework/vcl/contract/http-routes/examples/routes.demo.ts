import { num, str } from "@coldmind/model";
import { RouteTable, coerceFields, describeRoutes, get, httpController, post, routesOf } from "@coldmind/http-routes";

// A route group declared as a class: same schemas as an rpc contract, plus method and path.
@httpController("orders", { doc: "Order handling" })
class Orders {
  @get("/orders/:id", { params: { id: str() }, query: { verbose: num().default(0) }, result: { sku: str() }, access: "public" })
  get() {}

  @get("/orders/latest", { access: "public" })
  latest() {}

  @post("/orders", { body: { sku: str() }, status: 201 })
  create() {}
}

const table = new RouteTable([routesOf(Orders)]);

// The table decides which route a request is for; the topic is fixed per route.
for (const [method, path] of [["GET", "/orders/latest"], ["GET", "/orders/42"], ["POST", "/orders/42"], ["GET", "/nope"], ["HEAD", "/orders/7"]] as const) {
  const m = table.match(method, path);
  console.log(method, path, "->", m.kind === "match" ? `${m.route.topic} ${JSON.stringify(m.params)}${m.viaHead ? " (via GET)" : ""}` : m.kind === "method-not-allowed" ? `405 Allow: ${m.allow.join(", ")}` : m.kind);
}

// Query strings arrive as text; coerce by field kind before validation.
console.log(coerceFields({ verbose: num().default(0) }, { verbose: "2" }));

// The machine-readable contract.
console.log(JSON.stringify(describeRoutes(routesOf(Orders)).routes.get, null, 2).split("\n").slice(0, 8).join("\n"));
