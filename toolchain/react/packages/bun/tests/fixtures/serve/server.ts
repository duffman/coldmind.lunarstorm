import index from "@coldmind/devermind/tests/fixtures/serve/index.html";

const server = Bun.serve({ port: 0, development: { hmr: true }, routes: { "/": index } });
console.log(`url=${server.url}`);
