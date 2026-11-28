# Search, palette and shortcuts

## The command palette

Press `Ctrl+K` (or `Ctrl+Shift+P`, or click the search field in the title bar). A floating list opens with a search box. Type to narrow it; the groups are:

- **Commands**: open a view, new terminal, switch theme and so on.
- **Projects**: pick one to switch to it.
- **Tasks** and **Notes**: titles from every project that is not archived. Picking one switches to its project first.
- **In notes**: notes whose text contains all of your words (at least two characters), with an excerpt of the match.
- **Commits**: commits of the selected project.

Up and Down (or Tab) move, Enter runs the highlighted row, and Escape or a click outside closes it. Matching prefers an exact match, then a start of the text or a word, then anything that contains it, then a loose match.

Choosing a view from the palette always opens it; it never closes it the way its shortcut does when the view is already showing.

## Shortcuts

| Shortcut | What it does |
| --- | --- |
| `Ctrl+K` or `Ctrl+Shift+P` | Open the command palette |
| `F1` | Open or close this guide |
| `Ctrl+Shift+G` | Git: Changes (again to close) |
| `Ctrl+Shift+H` | Git: History (again to close) |
| `Ctrl+Shift+R` | Run commands |
| `Ctrl+Shift+M` | Timesheet |
| `Ctrl+Shift+L` | Credentials |
| `Ctrl+,` | Settings |
| `Ctrl+Shift+N` | New project (focuses the name box) |
| `` Ctrl+` `` | Show or hide the terminal |
| `` Ctrl+Shift+` `` | New terminal tab |

The view shortcuts open the view and press again to close it. They work from anywhere, even while a terminal has focus.

In the note editor and preview:

| Shortcut | What it does |
| --- | --- |
| `Ctrl+S` | Save and go to Preview |
| `Escape` | Close a suggestion list, or leave the editor |
| `Ctrl+F`, `F3`, `Shift+F3` | Search a rendered note, next, previous |
| `Ctrl+C`, `Ctrl+A` | Copy, or select all, in the preview |
| `Enter` or `Tab` | Insert the highlighted choice of a `[[`, `#` or `/` list |

## Where else to look

- Every shortcut above is also a palette command that shows its shortcut.
- Hovering a sidebar row shows its shortcut.
