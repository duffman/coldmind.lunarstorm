import { any, guard, list, obj, str } from "@coldmind/model";
import { contract } from "@coldmind/rpc";

/** Who a call runs as. Auth adapters produce it; `roles` are what `access: { roles }` checks. */
export type Principal = { id: string; roles: string[]; claims?: Record<string, unknown> };

/** What the client sees of a principal: id and roles. `claims` stay on the server. */
const publicPrincipal = obj({ id: str(), roles: list(str()) });
const isRecord = (v: unknown) => typeof v === "object" && v !== null && !Array.isArray(v);

/** Built-in `auth` controller, shared by Server and Client. `login` binds a verified principal to the session. */
export const authContract = contract("auth", {
  login: {
    args: { method: str(), credentials: guard(any<Record<string, unknown>>(), isRecord, "Expected an object") },
    result: { principal: publicPrincipal },
    access: "public",
  },
  logout: { args: {}, access: "public" },
  whoami: { args: {}, result: { principal: publicPrincipal.nullable() }, access: "public" },
});
