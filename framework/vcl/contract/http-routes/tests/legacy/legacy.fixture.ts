// Run by routes.decorators.test.ts under `experimentalDecorators` (this directory's tsconfig), so Bun emits the legacy dialect.
import { get, httpController, post, routeHandlersOf, routesOf } from "@coldmind/http-routes";
import { str } from "@coldmind/model";

@httpController("notes", { doc: "Notes" })
class Notes {
  constructor(private readonly base: string) {}

  @get("/notes/:id", { params: { id: str() }, access: "public" })
  list({ params }: { params: { id: string } }) {
    return `${this.base}-${params.id}`;
  }

  @post("/notes", { body: { text: str() } })
  add() {}
}

const handlers = routeHandlersOf(new Notes("base"));
const c = routesOf(Notes);
console.log(
  JSON.stringify({
    name: c.name,
    routes: Object.keys(c.routes),
    methods: Object.values(c.routes).map(r => r.method),
    doc: c.doc,
    listed: await handlers.list!({ params: { id: "1" } }, {}),
  }),
);
