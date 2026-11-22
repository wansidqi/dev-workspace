# Markdown preview demo

A test note that shows everything the note preview supports, following GitHub's
[basic writing and formatting syntax](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax).
Open it in **Preview**, then in **Edit** to compare the source with the result.

Jump to a section (these `#anchor` links scroll the note):

- [Headings](#headings)
- [Text styling](#text-styling)
- [Quotes and alerts](#quotes-and-alerts)
- [Code](#code)
- [Colors](#colors)
- [Links](#links)
- [Line breaks](#line-breaks)
- [Lists and tasks](#lists-and-tasks)
- [Tables](#tables)
- [Footnotes](#footnotes)
- [Emoji](#emoji)
- [Hidden comments](#hidden-comments)
- [Escaping](#escaping)
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

## Not supported yet

These are in GitHub's syntax but not in the preview yet. They show as plain text or alt text:

![Alt text for an image](https://example.com/image.png)

- `@octocat` mentions and `#123` issue references
- relative links to files, and `<a name="...">` custom anchors
- `<picture>` and other raw HTML (shown as text on purpose, notes are untrusted)

<script>alert("raw HTML is never run")</script>
