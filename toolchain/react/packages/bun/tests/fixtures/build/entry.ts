import { leaf } from "./leaf.ts";

// __MODE__ in a comment is replaced too: the P0 hook is a literal substitution.
const mode: string = __MODE__;
console.log(mode, leaf);
