"use strict";

const {
  webviewAssetPatch,
} = require("../../../../descriptor.js");
const {
  applyLinuxCompletedResumeRecoveryPatch,
} = require("../../../../impl/webview/index.js");

module.exports = [
  webviewAssetPatch({
    id: "linux-completed-resume-recovery",
    phase: "webview-asset",
    order: 1041,
    ciPolicy: "optional",
    pattern: /^app-initial~app-main~onboarding-page~hotkey-window-thread-page~quick-chat-window-page~chatg~[^.]+\.js$/,
    missingDescription: "app-server conversation manager bundle",
    skipDescription: "Linux completed resume recovery patch",
    apply: applyLinuxCompletedResumeRecoveryPatch,
  }),
];
