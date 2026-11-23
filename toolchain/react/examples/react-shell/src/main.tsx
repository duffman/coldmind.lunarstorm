import { startShell } from "@coldmind/devtools-shell";

startShell({
  root: document.getElementById("root")!,
  load: () => import("./App.tsx"),
});
