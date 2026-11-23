// Local preview of the browser host; the application remains a desktop model.
const files: Record<string, string> = {
  "/": "index.html",
  "/dist/application.main.js": "dist/application.main.js",
  "/dist/application.main.css": "dist/application.main.css",
};

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: Number(process.env.PORT || 5173),
  fetch(request) {
    const file = files[new URL(request.url).pathname];
    return file ? new Response(Bun.file(file)) : new Response("Not found", { status: 404 });
  },
});
console.log(`Desktop preview: ${server.url}`);
