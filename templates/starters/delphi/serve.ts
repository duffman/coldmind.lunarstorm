// A deliberately small local server for the compiled mockup.
const files: Record<string, string> = {
  "/": "index.html",
  "/dist/main.js": "dist/main.js",
  "/dist/main.css": "dist/main.css",
};

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: Number(process.env.PORT || 5173),
  fetch(request) {
    const path = files[new URL(request.url).pathname];
    return path ? new Response(Bun.file(path)) : new Response("Not found", { status: 404 });
  },
});
console.log(`Desktop mockup: ${server.url}`);
