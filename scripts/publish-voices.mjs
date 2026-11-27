// Dump the JP voices and deploy them to Cloudflare Pages when they changed.
//   npm run publish:voices [-- --force] [-- --dry-run]
// deploy.local.json (gitignored): {"project": "<Pages project>", "url": "https://<site>", "jp_root": "<JP client folder>"}
// Cloudflare login: `npx wrangler login` once, or CLOUDFLARE_API_TOKEN.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const force = process.argv.includes("--force");
const dryRun = process.argv.includes("--dry-run");
const cfg = JSON.parse(readFileSync("deploy.local.json", "utf8"));
for (const key of ["project", "url", "jp_root"]) {
  if (!cfg[key]) throw new Error(`deploy.local.json: "${key}" is missing`);
}
const repo = JSON.parse(readFileSync("package.json", "utf8")).repository.replace(/^github:/, "");

const today = new Date().toLocaleDateString("sv"); // YYYY-MM-DD, local time

const site = execFileSync(
  "cargo",
  ["run", "--release", "-q", "--manifest-path", "src-tauri/Cargo.toml", "-p", "mmvp-dumper", "--", cfg.jp_root, "--date", today, "--out", "work/pages", "--repo", repo],
  { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8" },
).trim();
const next = JSON.parse(readFileSync(join(site, "latest.json"), "utf8"));
const live = await fetch(`${cfg.url.replace(/\/$/, "")}/latest.json`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
console.log(`live: ${live ? `${live.date} ${live.manifest_sha1}` : "-"}\nnew:  ${next.date} ${next.manifest_sha1}`);

if (live?.manifest_sha1 === next.manifest_sha1 && !force) {
  console.log("voices unchanged: nothing to deploy");
} else if (dryRun) {
  console.log(`would deploy ${site} to Pages project ${cfg.project}`);
} else {
  // wrangler's own entry point, run by this Node: no shell, so paths with spaces are safe
  const wrangler = join("node_modules", "wrangler", "bin", "wrangler.js");
  execFileSync(process.execPath, [wrangler, "pages", "deploy", site, "--project-name", cfg.project, "--branch", "main", "--commit-dirty=true"], { stdio: "inherit" });
}
