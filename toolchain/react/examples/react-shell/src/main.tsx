import { startShell } from "@coldmind/devermind-shell";

startShell({
  root: document.getElementById("root")!,
  load: () => import("./App.tsx"),
});
