declare const __MODE__: string;
declare const process: { env: Record<string, string | undefined> };

// __MODE__ in a comment stays.
const label = "__MODE__ in a string stays";
const local = (__MODE__: string) => __MODE__;
export const result = {
  mode: __MODE__,
  label,
  local: local("param"),
  env: process.env.NODE_ENV,
  shorthand: { __MODE__ },
};
