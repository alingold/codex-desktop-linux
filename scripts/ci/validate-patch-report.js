#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const {
  requiredPatchNamesForProfile,
} = require("../patches/runner.js");
const {
  SUCCESS_STATUSES,
  criticalFailuresFromReport,
  optionalDriftFromReport,
} = require("../lib/patch-report.js");

function usage() {
  return [
    "Usage: validate-patch-report.js <patch-report.json> [--profile upstream-build]",
    "       [--require-enabled-feature FEATURE_ID] [--require-success PATCH_NAME]",
  ].join("\n");
}

function uniqueStrings(values) {
  const list = Array.isArray(values) ? values : [values];
  return [...new Set(list.filter((value) => typeof value === "string" && value.length > 0))];
}

function parseArgs(argv) {
  let profile = "upstream-build";
  const requiredEnabledFeatures = [];
  const requiredSuccessfulPatches = [];
  const positional = [];

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--profile") {
      profile = argv[index + 1];
      if (!profile) {
        throw new Error(usage());
      }
      index += 1;
    } else if (arg === "--require-enabled-feature") {
      const featureId = argv[index + 1];
      if (!featureId) {
        throw new Error(usage());
      }
      requiredEnabledFeatures.push(featureId);
      index += 1;
    } else if (arg === "--require-success") {
      const patchName = argv[index + 1];
      if (!patchName) {
        throw new Error(usage());
      }
      requiredSuccessfulPatches.push(patchName);
      index += 1;
    } else if (arg === "--help" || arg === "-h") {
      console.log(usage());
      process.exit(0);
    } else if (arg.startsWith("--")) {
      throw new Error(`Unknown option: ${arg}\n${usage()}`);
    } else {
      positional.push(arg);
    }
  }

  if (positional.length !== 1) {
    throw new Error(usage());
  }

  return {
    profile,
    reportPath: positional[0],
    requirements: {
      requiredEnabledFeatures: uniqueStrings(requiredEnabledFeatures),
      requiredSuccessfulPatches: uniqueStrings(requiredSuccessfulPatches),
    },
  };
}

function readReport(reportPath) {
  const raw = fs.readFileSync(reportPath, "utf8");
  const report = JSON.parse(raw);
  if (report == null || typeof report !== "object" || !Array.isArray(report.patches)) {
    throw new Error(`Invalid patch report: ${reportPath}`);
  }
  return report;
}

function validateReport(report, profile, requirements = {}) {
  const requiredNames = requiredPatchNamesForProfile(profile);
  const patches = Array.isArray(report?.patches) ? report.patches : [];
  const patchesByName = new Map(patches.map((patch) => [patch.name, patch]));
  const enabledFeatures = new Set(Array.isArray(report.enabledFeatures) ? report.enabledFeatures : []);
  const requiredEnabledFeatures = uniqueStrings(requirements.requiredEnabledFeatures ?? []);
  const requiredSuccessfulPatches = uniqueStrings(requirements.requiredSuccessfulPatches ?? []);
  const failures = [];

  // A required patch that never ran leaves no report entry, so the
  // report-driven check below cannot see it — catch it by name first.
  for (const name of requiredNames) {
    if (!patchesByName.has(name)) {
      failures.push(`${name}: missing from patch report`);
    }
  }

  // Shared predicate with the local build gate (patch-linux-window-ui.js
  // --enforce-critical): any recorded critical patch with a non-success,
  // applicable status fails validation.
  for (const failure of criticalFailuresFromReport(report)) {
    failures.push(`${failure.name}: ${failure.status}${failure.reason ? ` (${failure.reason})` : ""}`);
  }

  for (const featureId of requiredEnabledFeatures) {
    if (!enabledFeatures.has(featureId)) {
      failures.push(`feature ${featureId}: not enabled in patch report`);
    }
  }

  for (const name of requiredSuccessfulPatches) {
    const patch = patchesByName.get(name);
    if (patch == null) {
      failures.push(`${name}: missing from patch report`);
      continue;
    }
    if (!SUCCESS_STATUSES.has(patch.status)) {
      failures.push(`${name}: ${patch.status}${patch.reason ? ` (${patch.reason})` : ""}`);
    }
  }

  return failures;
}

function printOptionalDrift(report) {
  const drift = optionalDriftFromReport(report);
  if (drift.length === 0) {
    return;
  }
  console.warn(`Optional patch drift (${drift.length}, non-failing):`);
  for (const item of drift) {
    console.warn(`- ${item.name}: ${item.status}${item.reason ? ` (${item.reason})` : ""}`);
  }
}

function main() {
  try {
    const { profile, reportPath, requirements } = parseArgs(process.argv.slice(2));
    const report = readReport(reportPath);
    printOptionalDrift(report);
    const failures = validateReport(report, profile, requirements);
    if (failures.length > 0) {
      console.error(`Required patch validation failed for profile ${profile}:`);
      for (const failure of failures) {
        console.error(`- ${failure}`);
      }
      process.exit(1);
    }
    console.log(`Required patch validation passed for profile ${profile}.`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}

module.exports = {
  SUCCESS_STATUSES,
  readReport,
  validateReport,
};
