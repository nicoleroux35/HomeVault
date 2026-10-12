#!/usr/bin/env node

/**
 * Pre-compress the built frontend so the server never has to.
 *
 * HomeVault embeds the whole `static/` tree into the binary at compile time
 * (`include_dir!` in `src/handlers/spa.rs`) and used to rely on
 * `tower_http::compression::CompressionLayer` alone, which re-compresses every
 * response on every request — brotli on a 700 KB bundle, per hit, on hardware
 * that is often a low-power board. Compressing once here instead means the
 * server only has to pick the right file.
 *
 * For every compressible file under `static/dist` this writes a `.br` and a
 * `.gz` sibling *next to it*, which the same `include_dir!` picks up on the
 * next build. `src/handlers/spa.rs` then serves the `.br` bytes when the client
 * sends `Accept-Encoding: br` (and marks the response `Content-Encoding: br`,
 * which is also what makes `CompressionLayer` leave it alone) rather than
 * compressing anything itself.
 *
 * Runs automatically as npm's `postbuild`, i.e. after every `vite build`, so
 * the siblings can never drift from the bundle they belong to. A plain
 * re-run is a no-op (see `isUpToDate`).
 */

import { readFileSync, writeFileSync, readdirSync, statSync, rmSync, existsSync } from 'fs';
import { join, dirname, extname } from 'path';
import { fileURLToPath } from 'url';
import { brotliCompressSync, gzipSync, constants } from 'zlib';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const DIST_DIR = join(__dirname, '../../static/dist');

/**
 * Text-ish extensions worth compressing. Raster images (`webp`, `png`, `jpg`)
 * and `woff2` are already compressed internally — re-compressing them costs
 * build time and can even grow the file, which `writeIfSmaller` would then
 * reject anyway.
 */
const COMPRESSIBLE = new Set([
  '.js',
  '.mjs',
  '.cjs',
  '.css',
  '.html',
  '.htm',
  '.svg',
  '.json',
  '.webmanifest',
  '.xml',
  '.txt',
]);

/** Below this, the encoding overhead outweighs the saving. */
const MIN_SIZE_BYTES = 1024;

/**
 * Brotli quality. Quality 11 is the smallest output but is dramatically
 * slower past ~200 KB, and this runs on every build — so the handful of large
 * bundles take the fast-but-still-excellent q9 and everything else gets q11.
 */
function brotliQuality(size) {
  return size > 200 * 1024 ? 9 : 11;
}

/** Recursively list every file under `dir` (absolute paths). */
function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walk(full));
    } else if (entry.isFile()) {
      out.push(full);
    }
  }
  return out;
}

/** True when `variant` already exists and is newer than its source file. */
function isUpToDate(source, variant) {
  if (!existsSync(variant)) return false;
  try {
    return statSync(variant).mtimeMs >= statSync(source).mtimeMs;
  } catch {
    return false;
  }
}

/** Write `data` to `target` only if it really is smaller than the original. */
function writeIfSmaller(target, data, originalSize) {
  if (data.length >= originalSize) {
    // Never ship a "compressed" variant that is not actually smaller.
    if (existsSync(target)) rmSync(target);
    return 0;
  }
  writeFileSync(target, data);
  return data.length;
}

function main() {
  if (!existsSync(DIST_DIR)) {
    console.log(`No build output at ${DIST_DIR} — nothing to pre-compress.`);
    return;
  }

  const files = walk(DIST_DIR);
  // `.br` / `.gz` siblings from a previous run are outputs, never inputs.
  const sources = files.filter((f) => {
    const ext = extname(f);
    return ext !== '.br' && ext !== '.gz' && COMPRESSIBLE.has(ext);
  });

  // Drop orphaned variants (their source disappeared) so a stale encoded body
  // can never be embedded by the next `cargo build`.
  const sourceSet = new Set(sources);
  for (const file of files) {
    if (extname(file) !== '.br' && extname(file) !== '.gz') continue;
    if (!sourceSet.has(file.slice(0, -3))) rmSync(file);
  }

  let rawTotal = 0;
  let brotliTotal = 0;
  let gzipTotal = 0;
  let encoded = 0;
  let skipped = 0;

  for (const source of sources) {
    const size = statSync(source).size;
    if (size < MIN_SIZE_BYTES) continue;

    const brPath = `${source}.br`;
    const gzPath = `${source}.gz`;

    if (isUpToDate(source, brPath) && isUpToDate(source, gzPath)) {
      rawTotal += size;
      brotliTotal += statSync(brPath).size;
      gzipTotal += statSync(gzPath).size;
      skipped++;
      continue;
    }

    const contents = readFileSync(source);
    const brotli = brotliCompressSync(contents, {
      params: { [constants.BROTLI_PARAM_QUALITY]: brotliQuality(size) },
    });
    const gzip = gzipSync(contents, { level: 9 });

    const brWritten = writeIfSmaller(brPath, brotli, size);
    const gzWritten = writeIfSmaller(gzPath, gzip, size);

    rawTotal += size;
    brotliTotal += brWritten;
    gzipTotal += gzWritten;
    encoded++;
  }

  const pct = (total) => (rawTotal === 0 ? 0 : Math.round((1 - total / rawTotal) * 100));
  console.log(
    `Pre-compressed ${encoded} file(s)` +
      (skipped ? `, reused ${skipped}` : '') +
      ` — ${(rawTotal / 1024).toFixed(0)} KB raw, ` +
      `${(brotliTotal / 1024).toFixed(0)} KB br (-${pct(brotliTotal)}%), ` +
      `${(gzipTotal / 1024).toFixed(0)} KB gzip (-${pct(gzipTotal)}%)`
  );

  if (rawTotal === 0) {
    console.log(`Nothing compressible under ${DIST_DIR}.`);
  } else if (brotliTotal === 0) {
    // Reaching here means every file was below MIN_SIZE_BYTES or every
    // "compressed" output was larger — the server then falls back to
    // on-the-fly compression, which is correct but slower.
    console.log('No variant was smaller than its source — the server will compress on the fly.');
  }
}

main();
