/**
 * Build ekstensi.
 *
 * KENAPA BUKAN SATU PANGGILAN VITE
 * Side panel adalah halaman HTML biasa dan boleh berupa modul ES. Content
 * script MV3 TIDAK boleh berupa modul, jadi harus dibundel sebagai IIFE tanpa
 * import apa pun. Rollup hanya bisa menghasilkan satu format per build, jadi
 * bagian bagian itu dibangun terpisah lalu digabung ke satu direktori dist.
 */

import { build } from "vite";
import { cp, mkdir, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const dist = resolve(root, "dist");

const jsx = { jsx: "automatic", jsxImportSource: "preact" };

/** Halaman side panel. Modul ES, dengan HTML sebagai titik masuk. */
async function buildPanel() {
  await build({
    root: resolve(root, "src"),
    base: "./",
    esbuild: jsx,
    logLevel: "warn",
    build: {
      outDir: dist,
      emptyOutDir: false,
      target: "chrome116",
      rollupOptions: {
        input: resolve(root, "src/sidepanel/index.html"),
        output: {
          entryFileNames: "sidepanel/[name].js",
          chunkFileNames: "sidepanel/[name].js",
          assetFileNames: "sidepanel/[name][extname]",
        },
      },
    },
  });
}

/** Skrip yang harus berupa IIFE. Satu build per berkas. */
async function buildScript(entry, outName) {
  await build({
    root,
    esbuild: jsx,
    logLevel: "warn",
    build: {
      outDir: dist,
      emptyOutDir: false,
      target: "chrome116",
      rollupOptions: {
        input: resolve(root, entry),
        output: {
          format: "iife",
          entryFileNames: outName,
          // IIFE tidak boleh dipecah jadi beberapa berkas.
          inlineDynamicImports: true,
        },
      },
    },
  });
}

async function main() {
  if (existsSync(dist)) await rm(dist, { recursive: true });
  await mkdir(dist, { recursive: true });

  await buildPanel();
  await buildScript("src/content/chart-bridge.ts", "content/chart-bridge.js");
  await buildScript("src/content/relay.ts", "content/relay.js");
  await buildScript("src/background/service-worker.ts", "background/service-worker.js");

  await cp(resolve(root, "manifest.json"), resolve(dist, "manifest.json"));


  console.log("  ekstensi dibangun di extension/dist");
  console.log("  muat lewat chrome://extensions, Load unpacked, pilih direktori itu");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
