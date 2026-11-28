# Timesheet

The timesheet turns your git commits into a monthly table of work, one table per project. Open it from **Timesheet** in the sidebar (`Ctrl+Shift+M`).

## How it works

- Pick a **month**. The app reads that month's commits by your git author name across every local and remote branch of the project's linked repositories. Merges are skipped, and lockfiles and generated output are left out of the changed-line counts.
- Commits are grouped into **one row per ticket**. A ticket is found in the message as `7267:`, `#7267 -` or `[7267]`.
- Each row gets an estimated number of hours from the work in its commits. The constants behind the estimate are in the timesheet's own settings form.
- A month that was never saved syncs the first time you open it.

## Editing

Every edit is saved at once.

- Click a title or the hours to edit them. The **-** and **+** buttons change hours in half-hour steps.
- Add or remove manual rows for work that has no commits.
- Expand a row to see its commits.
- **Remarks** holds free text for the month.
- **Sync from git** reads the commits again in the background. Hours you typed, titles you edited and manual rows survive it; only estimated hours are recalculated.
- A repository that cannot be read leaves its project's table as it was, with a message, instead of wiping the saved rows.

## Copying

**Copy all** puts the month on the clipboard as tab-separated text with `=SUM` formulas, ready to paste into a spreadsheet. It is plain text, so fonts and borders do not come with it. There is no export to a file or to Google Sheets.

## Settings form

Your name, role and title for the sheet, the git author names to match, how many blank rows to pad each table with, and the constants of the hours estimate. Projects can be switched off or reordered.

The timesheet is part of a workspace backup but not of a single project's export.
