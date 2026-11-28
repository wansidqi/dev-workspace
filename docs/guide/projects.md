# Projects

A project groups everything for one piece of work: its tasks, notes, files, credentials, repositories and terminals. Switching project switches all of them.

## The project list

- **Create.** The plus beside **Projects** in the sidebar, or `Ctrl+Shift+N`.
- **Switch.** Click a project. The app remembers the last project, task and whether the terminal was open, and restores them on the next start.
- **Rename or archive.** Hover a project row to see the edit and archive buttons. An archived project leaves the list and the search results.

## The project page

The top shows the project's name, when it was created, how many repositories it has, **Open in IDE** and **New task**. Under it, a strip shows how many tasks are To do, In progress, Blocked and Done.

Below that is the task tree (see [Tasks and subtasks](topic:tasks)). A side rail on the right holds three lists:

- **Notes.** The project's own notes, most recent first. Click one to open it. See [Notes](topic:notes).
- **Files.** Files attached to the project. See [Attachments](topic:attachments).
- **Repositories.** The git repositories linked to the project.

## Linking repositories

A project can link one or more git repositories.

- Type a path, or press **Browse** to pick a folder. A folder inside a repository resolves to the repository's root. The name defaults to the folder name.
- Unlink a repository from the same list. Commits you mapped to tasks from that repository are deleted with it, so the app asks first.
- A repository the app cannot read is reported with a warning rather than hidden.

Every git view covers all the repositories of the project at once. See [Git](topic:git).

## Open in IDE

**Open in IDE** runs the IDE command from [Settings](topic:settings) (`code` by default) on the project's first linked repository, or on its data folder when it has none. It starts your IDE directly, without a shell.

## Backup, export and import

A whole project can be exported to a folder and imported again as a new project. See [Settings and data](topic:settings). Credentials are never part of an export.
