"use strict";

const {
  webviewAssetPatch,
} = require("../../../../descriptor.js");
const {
  applyLinuxUnownedTurnClaimPatch,
} = require("../../../../impl/webview/index.js");

module.exports = [
  webviewAssetPatch({
    id: "linux-unowned-turn-claim",
    phase: "webview-asset",
    order: 1042,
    ciPolicy: "optional",
    pattern: /^app-initial~app-main~onboarding-page~hotkey-window-thread-page~quick-chat-window-page~chatg~[^.]+\.js$/,
    missingDescription: "app-server conversation manager bundle",
    skipDescription: "Linux unowned turn claim patch",
    apply: applyLinuxUnownedTurnClaimPatch,
  }),
];
