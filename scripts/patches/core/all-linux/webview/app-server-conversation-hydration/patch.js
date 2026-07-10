"use strict";

const {
  webviewAssetPatch,
} = require("../../../../descriptor.js");
const {
  applyLinuxAppServerConversationHydrationPatch,
} = require("../../../../impl/webview/index.js");

module.exports = [
  webviewAssetPatch({
    id: "linux-app-server-conversation-hydration",
    phase: "webview-asset",
    order: 1043,
    ciPolicy: "optional",
    pattern: /^app-initial~app-main~(?:worktree-init-v2-page~remote-conversation-page~new-thread-panel-page~o~.*|new-thread-panel-page~appgen-library-page~hotkey-window-thread-page~ho~glxlkd48-.*|pull-request-code-review~onboarding-page~hotkey-window-thread-page~cha~b76hmflu-.*)\.js$/,
    missingDescription: "app-server conversation manager bundle",
    skipDescription: "Linux app-server conversation hydration patch",
    apply: applyLinuxAppServerConversationHydrationPatch,
  }),
];
