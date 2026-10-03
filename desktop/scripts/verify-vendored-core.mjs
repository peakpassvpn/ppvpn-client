#!/usr/bin/env node

import { createHash } from "node:crypto";
import { createReadStream, readFileSync, statSync } from "node:fs";
import { isAbsolute, join, relative, resolve } from "node:path";

function fail(message) {
  console.error(`vendored core verification failed: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      fail(`invalid arguments: ${argv.join(" ")}`);
    }
    args[key.slice(2)] = value;
  }
  return args;
}

async function sha256(path) {
  const hash = createHash("sha256");
  await new Promise((resolveStream, rejectStream) => {
    const stream = createReadStream(path);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", resolveStream);
    stream.on("error", rejectStream);
  });
  return hash.digest("hex");
}

const args = parseArgs(process.argv.slice(2));
const vendorDir = resolve(args["vendor-dir"] ?? "");
const artifactKey = args.artifact;
const expectedVersion = args["expected-version"];

if (!args["vendor-dir"] || !artifactKey || !expectedVersion) {
  fail("--vendor-dir, --artifact and --expected-version are required");
}

const manifestPath = join(vendorDir, "manifest.json");
let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
} catch (error) {
  fail(`cannot read ${manifestPath}: ${error.message}`);
}

if (manifest.schema_version !== 1) {
  fail(`unsupported manifest schema ${manifest.schema_version}`);
}
if (manifest.core_version !== expectedVersion) {
  fail(`manifest version ${manifest.core_version} does not match ${expectedVersion}`);
}
if (manifest.source?.repository !== "peakpassvpn/ppvpn-core") {
  fail(`unexpected source repository ${manifest.source?.repository}`);
}
if (!/^[0-9a-f]{40}$/.test(manifest.source?.commit ?? "")) {
  fail("source commit must be a full Git SHA");
}

const artifact = manifest.artifacts?.[artifactKey];
if (!artifact || typeof artifact.path !== "string" || typeof artifact.sha256 !== "string") {
  fail(`manifest does not define artifact ${artifactKey}`);
}
if (!/^[0-9a-f]{64}$/.test(artifact.sha256)) {
  fail(`artifact ${artifactKey} has an invalid SHA-256`);
}

const artifactPath = resolve(vendorDir, artifact.path);
const relativePath = relative(vendorDir, artifactPath);
if (!relativePath || relativePath.startsWith("..") || isAbsolute(relativePath)) {
  fail(`artifact ${artifactKey} escapes the vendor directory`);
}

let artifactStat;
try {
  artifactStat = statSync(artifactPath);
} catch (error) {
  fail(`cannot stat ${artifactPath}: ${error.message}`);
}
if (!artifactStat.isFile()) {
  fail(`${artifactPath} is not a file`);
}
if (artifactStat.size !== artifact.size) {
  fail(`artifact ${artifactKey} size ${artifactStat.size} does not match ${artifact.size}`);
}

const actualSha256 = await sha256(artifactPath);
if (actualSha256 !== artifact.sha256) {
  fail(`artifact ${artifactKey} SHA-256 ${actualSha256} does not match ${artifact.sha256}`);
}

console.log(
  `verified ppvpn-core ${manifest.core_version} ${artifactKey} from ${manifest.source.commit}`,
);
