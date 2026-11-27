# Markdown preview demo

A test note that shows everything the note preview supports, following GitHub's
[basic writing and formatting syntax](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax).
Open it in **Preview**, then in **Edit** to compare the source with the result.

Most of it is GitHub's own syntax. A few parts are this app's additions, and GitHub
shows them as plain text or a plain quote: our own alert kinds and collapsible
alerts ([Quotes and alerts](#quotes-and-alerts)), references to notes and tasks
([References](#references)) and the slash commands of the editor
([Slash commands](#slash-commands)).

Jump to a section (these `#anchor` links scroll the note):

- [Headings](#headings)
- [Text styling](#text-styling)
- [Quotes and alerts](#quotes-and-alerts)
- [Code](#code)
- [Colors](#colors)
- [Links](#links)
- [References](#references)
- [Line breaks](#line-breaks)
- [Lists and tasks](#lists-and-tasks)
- [Tables](#tables)
- [Footnotes](#footnotes)
- [Emoji](#emoji)
- [Hidden comments](#hidden-comments)
- [Escaping](#escaping)
- [Slash commands](#slash-commands)
- [Not supported yet](#not-supported-yet)

## Headings

# A first-level heading

## A second-level heading

### A third-level heading

#### A fourth-level heading

##### A fifth-level heading

###### A sixth-level heading

Levels 1 and 2 get a rule underneath, like GitHub.

## Text styling

**This is bold text** and __this is also bold__.

_This text is italicized_ and *so is this*.

~~This was mistaken text~~

**This text is _extremely_ important**

***All this text is important***

This is a <sub>subscript</sub> and this is a <sup>superscript</sup>: H<sub>2</sub>O, x<sup>2</sup>, E = mc<sup>2</sup>.

This is <ins>underlined</ins> text.

## Quotes and alerts

Text that is not a quote.

> Text that is a quote. It is indented with a bar on the left.
>
> A quote can have several paragraphs and **styling**.

> [!NOTE]
> Useful information that users should know, even when skimming content.

> [!TIP]
> Helpful advice for doing things better or more easily.

> [!IMPORTANT]
> Key information users need to know to achieve their goal.

> [!WARNING]
> Urgent info that needs immediate user attention to avoid problems.

> [!CAUTION]
> Advises about risks or negative outcomes of certain actions.

Three more kinds are this app's own. Each has its own icon, so the type never
depends on colour alone. GitHub shows them as a plain quote:

> [!TODO]
> Write the release notes before Friday.

> [!BUG]
> The save button does nothing when the note is empty.

> [!QUESTION]
> Should a folded alert remember its state between sessions?

Any alert can fold. A `-` after the kind starts it folded and a `+` starts it
open; either way, click its header to flip it (the chevron shows which way it is):

> [!NOTE]-
> This note starts folded. Click its header to open it.

> [!WARNING]+
> This warning starts open. Click its header to fold it.

## Code

Use `git status` to list all new or changed files that haven't yet been committed.

A fenced block with no language:

```
git status
git add
git commit
```

SQL gets syntax highlighting, and every block has a copy button:

```sql
-- Open tasks per project
SELECT p.name, COUNT(*) AS open_tasks
FROM tasks t
JOIN projects p ON p.id = t.project_id
WHERE t.status <> 'done' AND t.due_at < 1700000000
GROUP BY p.name
ORDER BY open_tasks DESC
LIMIT 10;
```

```rust
fn main() {
    // A comment
    let answer: u32 = 42;
    println!("The answer is {answer}");
}
```

```json
{ "name": "devspace", "local_first": true, "version": 1 }
```

## Colors

The background color is `#ffffff` for light mode and `#000000` for dark mode.

| Color model | Example                 |
| ----------- | ----------------------- |
| HEX         | `#0969DA`               |
| RGB         | `rgb(9, 105, 218)`      |
| HSL         | `hsl(212, 92%, 45%)`    |

A few more: `#2da44e`, `#cf222e`, `rgb(255, 193, 7)`, `hsl(300, 70%, 40%)`.

These are **not** colors, so they stay plain code: `#fff` (shorthand), ` #0969DA` (leading space), `rgb(1, 2)` (two channels).

## Links

- An inline link: [GitHub Pages](https://pages.github.com/)
- A bare URL becomes a link: https://github.com
- An email link: [say hello](mailto:someone@example.com)
- A section link: [back to the top](#markdown-preview-demo) or [jump to Tables](#tables)
- A relative link, which is refused with a message: [progress notes](docs/progress.md)

Links open in your default browser. Only `http`, `https` and `mailto` are allowed.

### Repeated headings

Headings with the same text get `-1`, `-2` ... added to their anchor.

### Repeated headings

This is the second one: [first](#repeated-headings) and [second](#repeated-headings-1).

## References

Notes and task descriptions can point at other notes and tasks of the **same
project**. Click one to open it. These are this app's own syntax: GitHub shows the
brackets and the `#` as plain text.

### By title

A reference is a title between double brackets:

- A note by its title: [[Markdown preview demo]] (this works when the note is called that)
- With your own label: [[Markdown preview demo|back to the top of this demo]]
- A subtask by the tasks it hangs under, joined with `›` (a typed `>` works too): [[Release › Deploy › Plan]] or [[Deploy > Plan]]

How a reference looks tells you whether it points at something:

- One that finds its note or task is drawn as a tinted chip.
- One that finds nothing is muted with a wavy underline, like this one: [[A note that does not exist]]. It only means the title is wrong or the note was deleted.

The examples above that name tasks of your own project (`Release`, `Deploy`,
`Plan`) show as broken until you have tasks with those names. If a title matches
both a note and a task, the note wins. A bare title prefers the top-level task, and
a longer chain picks the exact subtask.

### By task number

`#4` points at the task with that number: the small number beside a task's title
on its page, and the one commit messages use to find a task's commits. The preview
shows the number and the task's title, and strikes it through when the task is
Done.

- Try it with a task of this project (use one of your own numbers): #1
- The same through brackets: [[#1]]

A number that is no task of this project stays plain text, so numbers in prose are
left alone: issue #99999 is not turned into a link.

These are never references: anything inside code (`#4`, `[[Title]]`), a number
glued to a word (`C#4`, `abc#4`), digits followed by letters (`#4a`), more than
nine digits, and a heading marker (`# 4 things`).

### Typing a reference

In **Edit** you do not have to remember titles or numbers:

- Type `[[` and a list of the project's notes and tasks opens under the caret. It narrows as you type. A subtask shows the task it sits under, and typing a parent's name and `›` (or `>`) lists only its subtasks.
- Type `#` after a space or an opening bracket and a list of the project's tasks opens. Type digits to find a task by number, or a word to search titles. It stays closed for headings, for `C#`, and once you have typed a whole task number, so Enter still starts a new line.
- Up and Down move, Enter or Tab (or a click) inserts the choice, and Escape closes the list. Picking a subtask writes its whole chain, so the link opens that one and not another task with the same title.

## Line breaks

A single line break inside a paragraph
stays on the same line, like in a `.md` file.

Two spaces at the end of a line  
give a line break.

A backslash at the end of a line\
also gives one.

An HTML break tag<br/>works too.

A blank line starts a new paragraph.

## Lists and tasks

Unordered:

- George Washington
* John Adams
+ Thomas Jefferson

Ordered:

1. James Madison
2. James Monroe
3. John Quincy Adams

Nested:

1. First list item
   - First nested list item
     - Second nested list item
   - Another nested item
2. Second list item

Task list. Click a box to tick or untick it; the note text changes with it:

- [x] Write the markdown preview
- [x] Add clickable checkboxes
- [ ] Add images
- [ ] \(Optional) A task that starts with a parenthesis

Nested tasks are independent of their parent:

- [ ] Ship the release
  - [x] Build it
  - [ ] Test it
  - [ ] Announce it

Plain checkbox lines also work as tasks here (GitHub itself would show them as text):

[ ] check the metadata fix

[x] sync the local data with the server

[✅] no reflection if the thumbprint is scanned without records

[] a task written without a space

Each one needs its own paragraph (a blank line between), as in the note above.

## Tables

| Name        | Kind    | Notes                       |
| ----------- | ------- | --------------------------- |
| Projects    | core    | Create, edit, archive       |
| Tasks       | core    | Unlimited nesting           |
| Credentials | secure  | Encrypted, `XChaCha20`      |

## Footnotes

Here is a simple footnote[^1]. Footnotes are numbered in the order they are
used, and always appear at the bottom, wherever they are written.

A footnote can have more than one line[^long].

[^1]: My reference.
[^long]: To add line breaks within a footnote, add 2 spaces to the end of a line.  
    This is the second line.

## Emoji

Type `:name:` for an emoji: :+1: :tada: :rocket: :sparkles: :bug: :memo: :white_check_mark: :warning: :eyes: :fire:

Unknown names are left as written: :not_an_emoji:

## Hidden comments

There is a comment between these words: before <!-- This content will not appear in the rendered Markdown --> after.

<!--
A comment can also
span several lines.
-->

The comment above is invisible in Preview but visible in Edit.

## Escaping

Let's rename \*our-new-project\* to \*our-old-project\*. The stars are shown, not italics.

A backslash also protects other characters: \# not a heading, \[not a link\].

## Slash commands

In **Edit**, type `/` at the start of a line and a list of blocks opens. Keep
typing to narrow it, then press Enter or Tab (or click) to insert one. Escape
closes it. Each block is the same one the toolbar above the editor makes, except
for the alerts.

| Type                                                  | Inserts                          |
| ----------------------------------------------------- | -------------------------------- |
| `/heading`                                            | A heading                        |
| `/table`                                              | A table                          |
| `/code`, `/sql`                                       | A fenced code block, or an SQL one |
| `/tasks`                                              | A task list (`/todo` finds it too) |
| `/bullets`, `/numbered`                               | A bulleted or a numbered list    |
| `/quote`                                              | A quote                          |
| `/note`, `/tip`, `/important`, `/warning`, `/caution` | One of GitHub's alerts           |
| `/to-do`, `/bug`, `/question`                         | One of this app's alerts         |
| `/foldable`                                           | A collapsible alert              |
| `/link`, `/divider`                                   | A link, or a horizontal rule     |

`/alert` lists every alert. A word finds a command by its name or a keyword, so
`/fold` finds the collapsible alert and `/list` finds the three kinds of list.

The list stays closed where a slash is probably something else: in the middle of a
line, on an indented line, in a path such as `/usr/bin`, and inside a fenced code
block.

Try it on the empty line below: switch to Edit, put the caret on it and type `/`.


## Not supported yet

These are in GitHub's syntax but not in the preview yet. They show as plain text or alt text:

![Alt text for an image](https://example.com/image.png)

- `@octocat` mentions, and GitHub issue references (a `#123` here means a task of the project, see [References](#references))
- relative links to files, and `<a name="...">` custom anchors
- `<picture>` and other raw HTML (shown as text on purpose, notes are untrusted)

<script>alert("raw HTML is never run")</script>
