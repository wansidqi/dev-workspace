# Terminal and run commands

## Terminal

A panel on the right of the window, with tabs. Each project has its own set of tabs, and each tab starts in the project's folder.

- `` Ctrl+` `` shows or hides the panel. `` Ctrl+Shift+` `` opens a new tab.
- Switching project keeps every shell running. The sidebar shows how many terminals a project has. Closing a tab ends its shell.
- Double-click a tab to rename it. Drag the panel's left edge to resize it. **Split** stacks a second shell under the first in the same tab (two at most); **Unsplit** closes the focused one.
- Select with the mouse (double-click selects a word). Copy with `Ctrl+C` while text is selected, or `Ctrl+Shift+C`, or right-click. Paste with `Ctrl+V`, `Ctrl+Shift+V` or `Shift+Insert`. Mouse wheel scrolls back through the output.
- When a shell has exited, press Enter in the tab to start it again.
- How many lines of output are kept is a setting (see [Settings and data](topic:settings)). A running terminal keeps the size it started with.

The shortcuts work even while a terminal has focus.

Tab names and splits are not remembered when you close the app.

## Run commands

**Run commands** in the sidebar holds a project's saved commands (a dev server, a build, a test run) with a live log.

- **Add** a command with a name, the command line and an optional folder. Edit or delete it later.
- **Start** and **Stop** it. A status chip shows whether it is running, and the sidebar row shows how many are running.
- The log scrolls on its own, can be cleared, and **Copy log** puts it on the clipboard.
- Commands keep running while you look at another view.
- A command is started directly, not through a shell.

## Open in IDE

**Open in IDE** on the project page starts your IDE on the project. The command is set in [Settings and data](topic:settings).
