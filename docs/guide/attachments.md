# Attachments

You can attach files to a project, a task or a subtask at any depth: PDFs, images, documents, anything.

## Adding files

- On a task, open the **Attachments** tab. On the project, use the **Files** section of the side rail.
- **Add files** opens the system file picker. You can choose several files at once.
- For each attach you choose how it is kept:
  - **Copy into workspace.** The file is copied into the project's attachments folder (in the background, never overwriting another file). It stays available even if the original moves.
  - **Link by path.** Only the path is stored. Nothing is copied, so the file is not backed up with the project.

## Opening and removing

- **Open** uses the program your system has for that file type.
- **Show in folder** reveals it in the file manager.
- **Remove** (the cross) deletes a copied file from the workspace. A linked original is never touched.

Files that could run code (programs, scripts, installers, shortcuts and files with no extension) are never launched. They are only revealed in the file manager, because an attachment is untrusted. Copied attachments must stay inside the project's attachments folder, and the app refuses paths that point elsewhere.

Deleting a task also deletes the files it copied.

## Not available yet

There is no built-in PDF viewer or image preview yet: PDFs open in your default app. Renaming an attachment and dropping files onto the page are not supported either.
