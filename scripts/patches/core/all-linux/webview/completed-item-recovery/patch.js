"use strict";

const {
  webviewAssetPatch,
} = require("../../../../descriptor.js");
const {
  applyLinuxCompletedItemRecoveryPatch,
} = require("../../../../impl/webview/index.js");

module.exports = [
  webviewAssetPatch({
    id: "linux-completed-item-recovery",
    phase: "webview-asset",
    order: 1043,
    ciPolicy: "optional",
    pattern: /^app-initial~app-main~(?:worktree-init-v2-page~remote-conversation-page~new-thread-panel-page~o~.*|new-thread-panel-page~appgen-library-page~hotkey-window-thread-page~ho~glxlkd48-.*)\.js$/,
    missingDescription: "app-server conversation manager bundle",
    skipDescription: "Linux completed item recovery patch",
    apply: applyLinuxCompletedItemRecoveryPatch,
  }),
];
