// Dump the JP voices and deploy them to Cloudflare Pages when they changed.
//   npm run publish:voices [-- --force] [-- --dry-run]
// deploy.local.json (gitignored): {"project": "<Pages project>", "url": "<site>", "jp_root": "D:\\Nexon\\MabinogiMobile_JP"}
// Cloudflare login: `npx wrangler login` once, or CLOUDFLARE_API_TOKEN.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const force = process.argv.includes("--force");
const dryRun = process.argv.includes("--dry-run");
const cfg = JSON.parse(readFileSync("deploy.local.json", "utf8"));
const jpRoot = cfg.jp_root ?? "D:\\Nexon\\MabinogiMobile_JP";
const url = (/^https?:\/\//.test(cfg.url ?? "") ? cfg.url : `https://${cfg.url ?? `${cfg.project}.pages.dev`}`).replace(/\/$/, "");
const repo = JSON.parse(readFileSync("package.json", "utf8")).repository.replace(/^github:/, "");
const run = (cmd, args) => execFileSync(cmd, args, { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", shell: process.platform === "win32" });

const site = run("cargo", ["run", "--release", "-q", "-p", "mmvp-dumper", "--", jpRoot, "--out", "work/pages", "--repo", repo]).trim();
const next = JSON.parse(readFileSync(join(site, "latest.json"), "utf8"));
const live = await fetch(`${url}/latest.json`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
console.log(`live: ${live ? `${live.date} ${live.manifest_sha1}` : "-"}\nnew:  ${next.date} ${next.manifest_sha1}`);

if (live?.manifest_sha1 === next.manifest_sha1 && !force) {
  console.log("voices unchanged: nothing to deploy");
} else if (dryRun) {
  console.log(`would deploy ${site} to Pages project ${cfg.project}`);
} else {
  execFileSync("npx", ["wrangler", "pages", "deploy", site, "--project-name", cfg.project, "--branch", "main", "--commit-dirty=true"],
    { stdio: "inherit", shell: process.platform === "win32" });
}
