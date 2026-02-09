#!/usr/bin/env node
import { mkdirSync, writeFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");
const distDir = join(root, "dist");
const html = `<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Oxcer</title></head>
  <body>Oxcer desktop shell (main UI served via custom oxcer:// protocol)</body>
</html>
`;

mkdirSync(distDir, { recursive: true });
writeFileSync(join(distDir, "index.html"), html);
console.log("dist/index.html created");
