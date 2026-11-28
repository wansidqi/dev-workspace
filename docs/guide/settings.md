# Settings and data

Open **Settings** from the sidebar, with `Ctrl+,`, or from the palette. Changes apply at once and are remembered.

## Appearance

- **Theme.** Dark (the default), Light, or Follow system. Cycling it is also a palette command.
- **Transparency.** Normal or reduced. Reduced makes every surface opaque and turns off the window blur. It is also a palette command.
- **Motion.** Normal or reduced. Reduced turns the animations off. The app does not read the system setting, so this is manual.

## Terminal and IDE

- **Scrollback** is how many lines of output a new terminal keeps, from 100 to 100,000 (2,000 by default). A running terminal keeps the size it started with.
- **IDE command** is what **Open in IDE** runs, `code` by default. Write it like a command line; the project folder is added as one argument. See [Projects](topic:projects).

## Git

- **Background fetch** fetches every linked repository every 5 minutes. It is off by default.
- **GitHub** is off by default, and nothing contacts GitHub while it is off. Turn it on to see pull requests and CI status, and to sync issues as tasks (see [Tasks and subtasks](topic:tasks)). Sign in with GitHub's device flow (the app shows a code and opens GitHub's page) or paste a personal access token. The device flow needs an OAuth app with **Enable Device Flow** ticked: enter its client id here. The token is kept in your system keychain. **Sign out** removes it.

## Data

Your data lives in one folder. Settings → Data shows where, with a **Show in folder** button that selects the database in your file manager. Inside it:

- `app.db`: the database (projects, tasks, links, settings of the timesheet).
- `projects/<id>/notes` and `projects/<id>/attachments`: your notes as `.md` files and the files you copied in.
- `backups/`: workspace backups.
- `settings.txt`: these settings, as plain `key=value` lines.

## Backup, export and import

- **Backup** copies the whole workspace (database and project files) into a new dated folder under `backups/`. It includes credentials, which are encrypted and unreadable without the keychain key of this computer.
- **Export** writes one project to a folder named `<project>.devspace-export`. It never includes credentials.
- **Import** reads such a folder as a **new** project. Every id is renumbered, so it never overwrites an existing project.

Results of these actions appear as a short message at the bottom right of the window.
