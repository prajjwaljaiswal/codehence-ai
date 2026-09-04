# Ticket import templates

`tickets-template.csv` is a starting point for the board's **Import** button
(Tasks → Import, then *Save an example sheet*). Open it in Excel, Numbers or a
text editor, replace the rows, and import the result — `.csv`, `.tsv`, `.xlsx`,
`.xlsm`, `.xls`, `.xlsb` and `.ods` all work.

## Columns

The first non-empty row is read as headings. Only `title` is required.

| Column | Meaning |
| --- | --- |
| `title` | What the ticket is called. A row without one is skipped. |
| `epic` | Grouping label, shown under the title on the card. |
| `description` | **Becomes the agent's task prompt**, falling back to the title when empty. The most useful column to fill in properly. |
| `priority` | `low`, `medium`, `high` or `critical`. Empty means `medium`. |

Headings are matched on their letters and digits alone, so `Epic`, `epic` and
`  EPIC  ` are the same column. A heading that is not understood is reported
rather than ignored, since a typo there would otherwise drop a whole column
without saying so.

These aliases also work: `task` and `summary` for `title`, `feature` for `epic`,
`details` and `notes` for `description`.

## Agents are not a column

Which agent implements a ticket is decided from what the ticket asks for, so
there is nothing to fill in. A row about tests goes to the Tester, one about a
crash to the Debugger, one about documentation to the Documenter, and anything
else to the Implementer.

That decision reads the title and the description together, which is another
reason to write a real description: `Fix the crash on an empty query` routes
better than `Search fix`.

Nothing is created until you confirm: the import previews the file first and
tells you which rows it cannot use, and why.
