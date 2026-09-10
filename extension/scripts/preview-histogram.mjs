import { createServer } from "vite";
import { fileURLToPath } from "node:url";
const server = await createServer({
  root: fileURLToPath(new URL("../", import.meta.url)),
  esbuild: { jsx: "automatic", jsxImportSource: "preact" },
  server: { host: "127.0.0.1", port: 5187, strictPort: true },
});
await server.listen();
console.log("Histogram fixture: http://127.0.0.1:5187/fixtures/histogram.html");
